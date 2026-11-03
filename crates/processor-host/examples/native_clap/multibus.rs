//! Separate source buses must reach the selected native configuration unchanged.

use std::{sync::Arc, time::Duration};

use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping, CompileContext,
    CompileLimits, DeviceSelector, EdgeSpec, GraphCompiler, GraphDocument, NodeConfig, NodeId,
    NodeSpec, ProviderBusKind, ProviderNodeConfig, ProviderPorts, RenderFormat,
};
use mixrelay_control_protocol::ExternalProcessorDescriptor;
use mixrelay_processor_host::{CatalogNodeResolver, ExternalProcessorFactory, ProcessorCatalog};
use mixrelay_render::{BlockProcessor, PeakMeterBank};

use super::qualification::NativeResources;

const FRAMES: u32 = 128;

pub(super) fn run(
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
    descriptor: &ExternalProcessorDescriptor,
) -> Result<(), String> {
    let ports = descriptor
        .layouts
        .iter()
        .find(|ports| {
            ports.inputs.len() == 2
                && ports.inputs[1].kind == ProviderBusKind::Sidechain
                && ports.total_input_channels() == Ok(4)
                && ports.total_output_channels() == Ok(2)
        })
        .ok_or("fixture sidechain layout was not discovered")?;
    let parameters = serde_json::json!({"parameterValues":{"1":0.5,"2":0.25,"3":0.0}});
    let graph = document(&descriptor.type_id, ports.clone(), parameters);
    let mut processor = prepare(catalog, controls, &graph)?;
    check_pcm(&mut processor)?;
    let state = controls
        .registry
        .snapshot_state(&NodeId::from("effect"), Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    drop(processor);
    if controls.registry.has_worker(&NodeId::from("effect")) {
        return Err("sidechain worker remains owned".into());
    }
    let saved = document(&descriptor.type_id, ports.clone(), state);
    let round_trip: GraphDocument =
        serde_json::from_slice(&serde_json::to_vec(&saved).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if saved != round_trip {
        return Err("bus selection changed during graph serialization".into());
    }
    let mut restored = prepare(catalog, controls, &round_trip)?;
    check_pcm(&mut restored)?;
    drop(restored);
    if controls.registry.has_worker(&NodeId::from("effect")) {
        return Err("restored sidechain worker remains owned".into());
    }
    println!(
        "{}",
        serde_json::json!({"nativeSidechain":true,"layoutId":ports.layout_id,
        "inputBuses":2,"processedBlocks":80,"assertedPcmBlocks":76,"stateRecall":true,"cleanup":"released"})
    );
    Ok(())
}

fn prepare(
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
    graph: &GraphDocument,
) -> Result<BlockProcessor, String> {
    let execution = GraphCompiler::new(CatalogNodeResolver::new(catalog.clone()))
        .compile(
            graph,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: FRAMES,
                },
                limits: CompileLimits::default(),
            },
        )
        .map_err(|error| error.to_string())?;
    let factory = ExternalProcessorFactory::with_controls(
        catalog.clone(),
        controls.registry.clone(),
        controls.files.clone(),
        controls.clock.clone(),
    );
    BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(PeakMeterBank::new(4, 4)),
        &factory,
    )
    .map_err(|error| error.to_string())
}

fn check_pcm(processor: &mut BlockProcessor) -> Result<(), String> {
    let mut previous = [0.0_f32; 2];
    for sequence in 0..40_u16 {
        let delta = f32::from(sequence) / 128.0;
        let main = [1.0 + delta, 0.5 + delta];
        let side = [0.25 - delta, 0.75 + delta];
        let main_pcm: Vec<_> = (0..FRAMES).flat_map(|_| main).collect();
        let side_pcm: Vec<_> = (0..FRAMES).flat_map(|_| side).collect();
        let sources: Vec<&[f32]> = processor
            .plan()
            .source_bindings()
            .iter()
            .map(|binding| {
                if binding.node_id().as_str() == "main" {
                    main_pcm.as_slice()
                } else {
                    side_pcm.as_slice()
                }
            })
            .collect();
        let mut output = vec![0.0; 256];
        processor
            .process_block(FRAMES, &sources, &mut [&mut output])
            .map_err(|error| error.to_string())?;
        if sequence >= 2
            && output.chunks_exact(2).any(|frame| {
                frame.iter().zip(previous).any(|(actual, expected)| {
                    !actual.is_finite() || (actual - expected).abs() > 1e-6
                })
            })
        {
            return Err(format!(
                "independent sidechain PCM mismatch at block {sequence}"
            ));
        }
        previous = [
            main[0] * 0.5 + side[0] * 0.5,
            main[1] * 0.25 + side[1] * 0.5,
        ];
        // Controlled IPC pacing only, not native-device or real-time jitter qualification.
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn document(type_id: &str, ports: ProviderPorts, parameters: serde_json::Value) -> GraphDocument {
    let nodes = vec![
        node(
            "main",
            NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::DefaultInput,
                output_channels: 2,
                gain_db: 0.0,
            }),
        ),
        node(
            "side",
            NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                selector: DeviceSelector::DefaultInput,
                output_channels: 2,
                gain_db: 0.0,
            }),
        ),
        node(
            "effect",
            NodeConfig::Provider(ProviderNodeConfig {
                type_id: type_id.into(),
                ports: Some(ports),
                parameters,
            }),
        ),
        node(
            "out",
            NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                selector: DeviceSelector::DefaultOutput,
                input_channels: 2,
            }),
        ),
    ];
    GraphDocument::new(
        1,
        nodes,
        vec![
            edge("main", "effect", "audio"),
            edge("side", "effect", "bus:1"),
            edge("effect", "out", "audio"),
        ],
    )
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: id.into(),
        label: None,
        control: mixrelay_audio_graph::NodeControl::default(),
        config,
    }
}

fn edge(source: &str, destination: &str, port: &str) -> EdgeSpec {
    EdgeSpec {
        id: format!("{source}-{destination}").as_str().into(),
        source: AudioEndpoint {
            node_id: source.into(),
            port_id: "audio".into(),
        },
        destination: AudioEndpoint {
            node_id: destination.into(),
            port_id: port.into(),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}
