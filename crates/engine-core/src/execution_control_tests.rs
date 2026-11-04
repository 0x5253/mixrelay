use super::*;
use mixrelay_audio_graph::{
    AudioEndpoint, ChannelLayout, ChannelMapping, EdgeSpec, NodeSpec, PortId, StreamSpec,
    VirtualEndpointConfig, VirtualEndpointId, VirtualEndpointReference,
};
use mixrelay_control_protocol::{
    GraphCatalogAction, GraphCatalogRequest, MeterSnapshot, NodeMeterSnapshot,
};

const DEVICE_A: &str = "31ab4e5a-44e0-4bab-89dd-906e07751ea1";
const DEVICE_B: &str = "31ab4e5a-44e0-4bab-89dd-906e07751ea2";

#[path = "execution_control_tests/capture_manifest.rs"]
mod capture_manifest;
#[path = "execution_control_tests/meters.rs"]
mod meters;
#[path = "execution_control_tests/shared_pcm.rs"]
mod shared_pcm;

fn device(id: &str, node: &str, reference: VirtualEndpointReference) -> NodeSpec {
    let stream = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from(node),
        label: None,
        config: NodeConfig::VirtualEndpoint(VirtualEndpointConfig {
            spec: VirtualEndpointSpec {
                id: VirtualEndpointId::from(id),
                display_name: id.to_owned(),
                render: Some(stream),
                capture: Some(stream),
                supported_sample_rates: vec![48_000],
                default_sample_rate: 48_000,
            },
            reference,
            gain_db: 0.0,
        }),
    }
}

fn route(source: (&str, &str), sink: (&str, &str)) -> EdgeSpec {
    EdgeSpec {
        id: "cable".into(),
        source: AudioEndpoint {
            node_id: source.0.into(),
            port_id: PortId::from(source.1),
        },
        destination: AudioEndpoint {
            node_id: sink.0.into(),
            port_id: PortId::from(sink.1),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

fn graph_a(revision: u64) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![device(
            DEVICE_A,
            "local",
            VirtualEndpointReference {
                render: true,
                capture: true,
            },
        )],
        vec![],
    )
}

fn graph_b(revision: u64) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![
            device(
                DEVICE_A,
                "local",
                VirtualEndpointReference {
                    render: true,
                    capture: false,
                },
            ),
            device(
                DEVICE_B,
                "output",
                VirtualEndpointReference {
                    render: false,
                    capture: true,
                },
            ),
        ],
        vec![route(("local", "from_os"), ("output", "to_os"))],
    )
}

fn controller_with_driver_devices() -> EngineGraphController {
    crate::endpoint_settings_tests::controller_with_inventory(
        GraphDocument::new(1, vec![], vec![]),
        &graph_b(1),
    )
}

fn select(engine: &EngineGraphController, action: GraphCatalogAction) {
    let catalog = engine.graph_catalog().unwrap();
    engine
        .mutate_graph_catalog(GraphCatalogRequest {
            expected_revision: engine.graph().revision,
            expected_catalog_revision: catalog.catalog_revision,
            action,
        })
        .unwrap();
}

fn assert_shared_pcm(processor: &mut BlockProcessor, expected_writers: &[&str]) {
    let frames = processor.plan().format().block_frames;
    let samples = frames as usize * 2;
    let signal: Vec<f32> = [0.25, -0.5, 0.75, -0.125]
        .into_iter()
        .cycle()
        .take(samples)
        .collect();
    let inputs: Vec<Vec<f32>> = processor
        .plan()
        .source_bindings()
        .iter()
        .map(|binding| {
            if binding
                .virtual_endpoint_id()
                .is_some_and(|id| id.as_str() == DEVICE_A)
            {
                signal.clone()
            } else {
                vec![0.0; samples]
            }
        })
        .collect();
    let mut outputs = vec![vec![0.0; samples]; processor.plan().sink_bindings().len()];
    // A device bus delays by its prepared quantum, not by the number of calls.
    for _ in 0..2 {
        processor
            .process_block(
                frames,
                &inputs.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                &mut outputs
                    .iter_mut()
                    .map(Vec::as_mut_slice)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
    }
    assert_eq!(
        processor
            .plan()
            .sink_bindings()
            .iter()
            .map(|binding| binding.virtual_endpoint_id().unwrap().as_str())
            .collect::<Vec<_>>(),
        expected_writers
    );
    assert_eq!(
        processor
            .plan()
            .source_bindings()
            .iter()
            .map(|binding| binding.virtual_endpoint_id().unwrap().as_str())
            .collect::<Vec<_>>(),
        [DEVICE_A]
    );
    for (binding, output) in processor.plan().sink_bindings().iter().zip(outputs) {
        match binding.virtual_endpoint_id().unwrap().as_str() {
            // A has no graph contribution route. Its basic OS audio return path belongs
            // to Driver and must not be copied to graph output a second time.
            DEVICE_A => assert_eq!(output, vec![0.0; samples]),
            DEVICE_B => assert_eq!(output, signal),
            _ => panic!("unexpected Driver sink"),
        }
    }
}
