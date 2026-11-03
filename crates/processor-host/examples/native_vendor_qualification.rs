#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a lab tool, not shipped: setup failures abort with their message"
)]
//! Opt-in qualification of one exact native VST3, CLAP, Audio Unit, or Linux LV2 effect.
//! Run with `--run <vst3|clap|au|lv2> <bundle path|AU component ID> [--parameter <id> <0..1>]` and optionally
//! `--editor <seconds> <new-absolute.png> [--drag <x> <y> <to-x> <to-y> | --editor-hold <1..30s>]`.
//! Linux uses standalone `--editor-hold <1..30s>` with real native window input.
//! macOS `--au-64-to-16 <exact component ID>` checks one accepted 64-channel AU layout,
//! explicit output routing, and saved-state fresh workers at 48k/128, 44.1k/256, and 48k/128.
//! AU is available only on macOS and uses an exact `aufx` or `aumf` component ID; LV2 is Linux-only.
//! Drag coordinates are logical pixels from the editor's top-left, not PNG pixels.
//! Pacing is not real-time measurement.
//! `--rescan-recovery <vst3|clap|lv2> <exact bundle>` is discovery-only. After each JSON
//! phase, the OS runner sends `denied` or `restored` on stdin within 30 seconds; this example
//! never changes permissions. The runner must restore its private QA bundle permissions on exit.

use std::{
    ffi::OsStr,
    fs::OpenOptions,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use mixrelay_audio_engine::{
    BlockProcessor, PeakMeterBank, ProcessorExchange, RuntimeControlUpdate,
};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, AudioDeviceInputConfig, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    ChannelRoute, CompileContext, CompileLimits, DeviceSelector, EdgeId, EdgeSpec, GraphCompiler,
    GraphDocument, NodeConfig, NodeControl, NodeId, NodeSpec, ProviderNodeConfig, ProviderPorts,
    RenderFormat,
};
use mixrelay_control_protocol::{
    EditorPointerKind, ExternalProcessorDescriptor, ProcessorDiscoveryStage, ProcessorEditorFrame,
    ProcessorEditorInput, ProcessorEditorState,
};
use mixrelay_processor_host::{
    CatalogNodeResolver, ExternalProcessorFactory, PLUGIN_PROBE_ARGUMENT, PLUGIN_SCAN_ARGUMENT,
    PLUGIN_WORKER_ARGUMENT, ProcessorCatalog, ProcessorControlRegistry, RackFormat, RackLocator,
};
use mixrelay_processor_state::StateRef;
use truce_rack_core::{info::PluginCategory, scanner::PluginScanner};

#[derive(Clone)]
struct NativeResources {
    registry: ProcessorControlRegistry,
    files: mixrelay_processor_host::ProcessorFiles,
    clock: Arc<dyn mixrelay_time::MonotonicClock>,
}

impl NativeResources {
    fn new(clock: Arc<dyn mixrelay_time::MonotonicClock>) -> Self {
        let files = mixrelay_processor_host::ProcessorFiles::default();
        let registry = ProcessorControlRegistry::new(Arc::new(files.clone()));
        Self {
            registry,
            files,
            clock,
        }
    }
}

const FORMAT_ENV: &str = "MIXRELAY_VENDOR_QUALIFICATION_FORMAT";
const PATH_ENV: &str = "MIXRELAY_VENDOR_QUALIFICATION_PATH";
const EFFECT: &str = "effect";
const FRAMES: u32 = 128;
const SAMPLES: usize = FRAMES as usize * 2;
const WARMUP: usize = 32;
const STEADY: usize = 8;
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: FRAMES,
};

struct Target {
    format: RackFormat,
    location: TargetLocation,
}

enum TargetLocation {
    Bundle(PathBuf),
    #[cfg(target_os = "macos")]
    AuComponentId(String),
}

impl Target {
    fn new(format: &str, requested_location: &OsStr) -> Result<Self, String> {
        let format = match RackFormat::parse(format)? {
            RackFormat::Vst3 => RackFormat::Vst3,
            RackFormat::Clap => RackFormat::Clap,
            #[cfg(target_os = "macos")]
            RackFormat::Au => RackFormat::Au,
            #[cfg(target_os = "linux")]
            RackFormat::Lv2 => RackFormat::Lv2,
            _ => return Err("qualification format is unavailable on this platform".into()),
        };
        let location = match format {
            #[cfg(target_os = "macos")]
            RackFormat::Au => {
                let component_id = requested_location
                    .to_str()
                    .ok_or("Audio Unit component ID must be valid UTF-8")?;
                if !valid_au_component_id(component_id) {
                    return Err("Audio Unit target must be an exact aufx/aumf FourCC ID".into());
                }
                TargetLocation::AuComponentId(component_id.to_owned())
            }
            _ => {
                let path = Path::new(requested_location)
                    .canonicalize()
                    .map_err(|error| error.to_string())?;
                let expected = format.argument();
                if path.extension().and_then(|extension| extension.to_str()) != Some(expected) {
                    return Err(format!("explicit bundle path must end in .{expected}"));
                }
                TargetLocation::Bundle(path)
            }
        };
        Ok(Self { format, location })
    }

    fn from_environment() -> Result<Self, String> {
        let format = std::env::var(FORMAT_ENV).map_err(|_| "qualification target is missing")?;
        let location = std::env::var_os(PATH_ENV).ok_or("qualification target ID is missing")?;
        Self::new(&format, location.as_os_str())
    }

    fn require_locator(&self, locator: &RackLocator) -> Result<(), String> {
        let location_matches = match &self.location {
            TargetLocation::Bundle(path) => &locator.path == path,
            #[cfg(target_os = "macos")]
            TargetLocation::AuComponentId(component_id) => {
                locator.unique_id.as_str() == component_id.as_str()
                    && locator.path.as_os_str().is_empty()
            }
        };
        if locator.format != self.format || !location_matches {
            return Err("qualification locator differs from the requested target".into());
        }
        Ok(())
    }

    fn request_argument(&self) -> &OsStr {
        match &self.location {
            TargetLocation::Bundle(path) => path.as_os_str(),
            #[cfg(target_os = "macos")]
            TargetLocation::AuComponentId(component_id) => OsStr::new(component_id),
        }
    }

    fn scan_argument(&self) -> PathBuf {
        self.request_argument().to_os_string().into()
    }

    fn require_scan_argument(&self, argument: &OsStr) -> Result<(), String> {
        if self.request_argument() == argument {
            Ok(())
        } else {
            Err("scanner target differs from the requested target".into())
        }
    }

    fn includes_effect(&self, unique_id: &str) -> bool {
        #[cfg(not(target_os = "macos"))]
        let _ = unique_id;

        match &self.location {
            TargetLocation::Bundle(_) => true,
            #[cfg(target_os = "macos")]
            TargetLocation::AuComponentId(component_id) => component_id == unique_id,
        }
    }

    fn publish_to_children(&self) {
        // SAFETY: this is called once, before discovery, plug-in loading, or thread creation.
        // Scanner, probe, and worker children inherit the exact target without later edits.
        unsafe {
            std::env::set_var(FORMAT_ENV, self.format.argument());
            std::env::set_var(PATH_ENV, self.request_argument());
        }
    }
}

#[cfg(target_os = "macos")]
fn valid_au_component_id(component_id: &str) -> bool {
    let mut fourccs = component_id.split(':');
    let Some(component_type) = fourccs.next() else {
        return false;
    };
    let Some(subtype) = fourccs.next() else {
        return false;
    };
    let Some(manufacturer) = fourccs.next() else {
        return false;
    };
    fourccs.next().is_none()
        && matches!(component_type, "aufx" | "aumf")
        && [component_type, subtype, manufacturer]
            .iter()
            .all(|fourcc| fourcc.len() == 4 && fourcc.is_ascii())
}

struct Request {
    target: Target,
    parameter: Option<(u32, f64)>,
    editor: Option<(u64, PathBuf)>,
    drag: Option<[f64; 4]>,
    editor_hold: Option<u64>,
}

fn parse_request(arguments: &[String]) -> Result<Request, String> {
    if arguments.len() < 3 || arguments[0] != "--run" {
        return Err("explicit --run <vst3|clap|au|lv2> <exact target> is required".into());
    }
    let target = Target::new(&arguments[1], OsStr::new(arguments[2].as_str()))?;
    let mut parameter = None;
    let mut editor = None;
    let mut drag = None;
    let mut editor_hold = None;
    let mut options = arguments[3..].iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--parameter" if parameter.is_none() => {
                let id = options.next().ok_or("missing parameter ID")?;
                let value = options.next().ok_or("missing parameter value")?;
                let id = id.parse::<u32>().map_err(|error| error.to_string())?;
                let value = value.parse::<f64>().map_err(|error| error.to_string())?;
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err("parameter value must be normalized between 0 and 1".into());
                }
                parameter = Some((id, value));
            }
            "--editor" if editor.is_none() && cfg!(target_os = "macos") => {
                let seconds = options.next().ok_or("missing editor wait seconds")?;
                let path = options.next().ok_or("missing editor PNG path")?;
                let seconds = seconds.parse::<u64>().map_err(|error| error.to_string())?;
                let path = PathBuf::from(path);
                if !(1..=30).contains(&seconds)
                    || !path.is_absolute()
                    || path.extension().and_then(|extension| extension.to_str()) != Some("png")
                {
                    return Err("editor needs 1..30 seconds and an absolute .png path".into());
                }
                editor = Some((seconds, path));
            }
            "--editor" => return Err("frame editor needs macOS and one PNG target".into()),
            "--drag" if drag.is_none() => {
                let mut points = [0.0_f64; 4];
                for coordinate in &mut points {
                    *coordinate = options
                        .next()
                        .ok_or("missing drag coordinate")?
                        .parse::<f64>()
                        .map_err(|error| error.to_string())?;
                    if !coordinate.is_finite() || !(0.0..=8192.0).contains(coordinate) {
                        return Err(
                            "drag coordinates must be finite logical pixels in 0..8192".into()
                        );
                    }
                }
                drag = Some(points);
            }
            "--editor-hold" if editor_hold.is_none() => {
                let seconds = options
                    .next()
                    .ok_or("missing editor hold seconds")?
                    .parse::<u64>()
                    .map_err(|error| error.to_string())?;
                if !(1..=30).contains(&seconds) {
                    return Err("editor hold needs 1..30 seconds".into());
                }
                editor_hold = Some(seconds);
            }
            _ => return Err(format!("unexpected qualification option: {option}")),
        }
    }
    if (drag.is_some() || (cfg!(target_os = "macos") && editor_hold.is_some())) && editor.is_none()
    {
        return Err("editor interaction requires --editor".into());
    }
    if drag.is_some() && editor_hold.is_some() {
        return Err("--drag and --editor-hold are exclusive".into());
    }
    Ok(Request {
        target,
        parameter,
        editor,
        drag,
        editor_hold,
    })
}

fn main() -> Result<(), String> {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match arguments.first().map(String::as_str) {
        Some(PLUGIN_SCAN_ARGUMENT) => scan(&arguments[1..]),
        Some(PLUGIN_PROBE_ARGUMENT) => probe(&arguments[1..]),
        Some(PLUGIN_WORKER_ARGUMENT) => worker(&arguments[1..]),
        Some("--rescan-recovery") => rescan_recovery(&arguments[1..]),
        Some("--au-64-to-16") => run_wide_au(&arguments[1..]),
        _ => run(&parse_request(&arguments)?),
    }
}

fn scan(arguments: &[String]) -> Result<(), String> {
    let [format, location] = arguments else {
        return Err("scanner requires one format and one exact target".into());
    };
    let target = Target::from_environment()?;
    target.require_scan_argument(OsStr::new(location))?;
    if format != target.format.argument() {
        println!("{{\"locators\":[],\"pathFailures\":[]}}");
        return Ok(());
    }
    let infos = match (&target.format, &target.location) {
        (RackFormat::Vst3, TargetLocation::Bundle(path)) => {
            truce_rack_vst3::Vst3Scanner::new().scan_path(path)
        }
        (RackFormat::Clap, TargetLocation::Bundle(path)) => {
            truce_rack_clap::ClapScanner::new().scan_path(path)
        }
        #[cfg(target_os = "linux")]
        (RackFormat::Lv2, TargetLocation::Bundle(path)) => {
            truce_rack_lv2::Lv2Scanner::new().scan_path(path)
        }
        #[cfg(target_os = "macos")]
        (RackFormat::Au, TargetLocation::AuComponentId(_)) => {
            truce_rack_au::AuScanner::new().scan()
        }
        _ => return Err("unsupported qualification format".into()),
    }
    .map_err(|error| error.to_string())?;
    let records = infos
        .into_iter()
        .filter(|info| {
            info.category == PluginCategory::Effect && target.includes_effect(&info.unique_id)
        })
        .map(|info| {
            serde_json::json!({
                "format": target.format.argument(), "uniqueId": info.unique_id,
                "path": info.path, "name": info.name, "vendor": info.vendor,
                "version": info.version, "category": "effect",
                "hasEditor": info.has_editor, "acceptsMidi": info.accepts_midi
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "locators": records,
            "pathFailures": []
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}

fn probe(arguments: &[String]) -> Result<(), String> {
    let [encoded] = arguments else {
        return Err("probe requires one locator".into());
    };
    let locator = RackLocator::decode(encoded)?;
    Target::from_environment()?.require_locator(&locator)?;
    mixrelay_processor_host::run_plugin_probe(locator)
}

fn worker(arguments: &[String]) -> Result<(), String> {
    let [encoded, _, _] = arguments else {
        return Err("worker requires locator and transport paths".into());
    };
    let locator = RackLocator::decode(encoded)?;
    Target::from_environment()?.require_locator(&locator)?;
    mixrelay_processor_host::run_plugin_worker(arguments)
}

fn stereo_layout(descriptor: &ExternalProcessorDescriptor) -> Result<ProviderPorts, String> {
    descriptor
        .layouts
        .iter()
        .find(|ports| {
            ports.total_input_channels() == Ok(2)
                && ports.total_output_channels() == Ok(2)
                && ports.inputs.len() == 1
                && ports.outputs.len() == 1
        })
        .cloned()
        .ok_or("native effect has no exact stereo input/output layout".into())
}

fn node(id: &str, config: NodeConfig) -> NodeSpec {
    NodeSpec {
        id: NodeId::from(id),
        label: None,
        control: NodeControl::default(),
        config,
    }
}

fn edge(id: &str, source: &str, destination: &str) -> EdgeSpec {
    EdgeSpec {
        id: EdgeId::from(id),
        source: AudioEndpoint {
            node_id: NodeId::from(source),
            port_id: AUDIO_PORT_ID.into(),
        },
        destination: AudioEndpoint {
            node_id: NodeId::from(destination),
            port_id: AUDIO_PORT_ID.into(),
        },
        channel_mapping: ChannelMapping::Auto,
    }
}

fn document(
    type_id: Option<&str>,
    ports: Option<&ProviderPorts>,
    state: serde_json::Value,
) -> GraphDocument {
    let mut nodes = vec![node(
        "in",
        NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
            selector: DeviceSelector::DefaultInput,
            output_channels: 2,
            gain_db: 0.0,
        }),
    )];
    if let Some(type_id) = type_id {
        nodes.push(node(
            EFFECT,
            NodeConfig::Provider(ProviderNodeConfig {
                type_id: type_id.into(),
                ports: ports.cloned(),
                parameters: state,
            }),
        ));
    }
    nodes.push(node(
        "out",
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 2,
        }),
    ));
    let edges = if type_id.is_some() {
        vec![
            edge("in-effect", "in", EFFECT),
            edge("effect-out", EFFECT, "out"),
        ]
    } else {
        vec![edge("in-out", "in", "out")]
    };
    GraphDocument::new(1, nodes, edges)
}

fn prepare(
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
    graph: &GraphDocument,
) -> Result<Box<BlockProcessor>, String> {
    let processor = prepare_at_format(catalog, controls, graph, FORMAT)?;
    if processor.plan().source_bindings().len() != 1
        || processor.plan().sink_bindings().len() != 1
        || processor.plan().source_bindings()[0].channels() != 2
        || processor.plan().sink_bindings()[0].channels() != 2
    {
        return Err("compiled graph is not one stereo input and output".into());
    }
    Ok(processor)
}

fn prepare_at_format(
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
    graph: &GraphDocument,
    format: RenderFormat,
) -> Result<Box<BlockProcessor>, String> {
    let execution = GraphCompiler::new(CatalogNodeResolver::new(catalog.clone()))
        .compile(
            graph,
            CompileContext {
                format,
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
    let processor = BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(PeakMeterBank::new(4, graph_input_width(graph))),
        &factory,
    )
    .map_err(|error| error.to_string())?;
    Ok(Box::new(processor))
}

fn graph_input_width(graph: &GraphDocument) -> usize {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.config {
            NodeConfig::AudioDeviceInput(input) => Some(usize::from(input.output_channels)),
            _ => None,
        })
        .max()
        .unwrap_or(2)
}

struct Harness {
    processor: Box<BlockProcessor>,
    exchange: ProcessorExchange,
}

impl Harness {
    fn new(
        catalog: &ProcessorCatalog,
        controls: &NativeResources,
        descriptor: &ExternalProcessorDescriptor,
        ports: &ProviderPorts,
        state: serde_json::Value,
    ) -> Result<Self, String> {
        let graph = document(Some(&descriptor.type_id), Some(ports), state);
        let processor = prepare(catalog, controls, &graph)?;
        let exchange = ProcessorExchange::new(1);
        exchange.observe_plan(Arc::clone(processor.plan()));
        Ok(Self {
            processor,
            exchange,
        })
    }

    fn block(&mut self, input: &[f32; SAMPLES]) -> Result<[f32; SAMPLES], String> {
        self.exchange.refresh_latency()?;
        self.exchange.activate_pending(&mut self.processor);
        let mut output = [0.0_f32; SAMPLES];
        self.processor
            .process_block(FRAMES, &[input], &mut [&mut output])
            .map_err(|error| error.to_string())?;
        std::thread::sleep(Duration::from_millis(10));
        self.exchange.reclaim_retired();
        Ok(output)
    }

    fn steady_pcm(&mut self) -> Result<Vec<f32>, String> {
        let input = input_block();
        for _ in 0..WARMUP {
            self.block(&input)?;
        }
        let mut pcm = Vec::with_capacity(STEADY * SAMPLES);
        for _ in 0..STEADY {
            let output = self.block(&input)?;
            if output.iter().any(|sample| !sample.is_finite()) {
                return Err("native worker produced non-finite PCM".into());
            }
            pcm.extend_from_slice(&output);
        }
        Ok(pcm)
    }

    fn set_effect_control(&mut self, revision: u64, control: NodeControl) -> Result<(), String> {
        self.exchange
            .publish_controls(
                self.processor.plan().controls(),
                revision,
                &[RuntimeControlUpdate {
                    node_id: NodeId::from(EFFECT),
                    control,
                    gain_db: None,
                }],
            )
            .map_err(|error| error.to_string())
    }
}

struct SignalGateEvidence {
    off: f32,
    muted: f32,
    resumed: f32,
}

fn pcm_peak(pcm: &[f32]) -> Result<f32, String> {
    if pcm.is_empty() || pcm.iter().any(|sample| !sample.is_finite()) {
        return Err("signal gate produced empty or non-finite PCM".into());
    }
    Ok(pcm.iter().map(|sample| sample.abs()).fold(0.0, f32::max))
}

fn qualify_signal_gates(
    harness: &mut Harness,
    controls: &NativeResources,
) -> Result<SignalGateEvidence, String> {
    let plan = Arc::clone(harness.processor.plan());
    let node = NodeId::from(EFFECT);
    let mut peaks = [0.0; 3];
    for ((revision, control), peak) in [
        (
            2,
            NodeControl {
                enabled: false,
                muted: false,
            },
        ),
        (
            3,
            NodeControl {
                enabled: true,
                muted: true,
            },
        ),
        (4, NodeControl::default()),
    ]
    .into_iter()
    .zip(&mut peaks)
    {
        harness.set_effect_control(revision, control)?;
        *peak = pcm_peak(&harness.steady_pcm()?)?;
        if !Arc::ptr_eq(&plan, harness.processor.plan()) || !controls.registry.has_worker(&node) {
            return Err("signal gate replaced the plan or removed its native worker".into());
        }
        if harness.exchange.revisions().active != revision {
            return Err("signal gate was not acknowledged by the audio reader".into());
        }
        if (!control.enabled || control.muted) && *peak != 0.0 {
            return Err(format!("signal gate passed nonzero PCM: {peak}"));
        }
    }
    if peaks[2] <= 0.000_01 {
        return Err("default native effect did not resume after OFF and mute".into());
    }
    Ok(SignalGateEvidence {
        off: peaks[0],
        muted: peaks[1],
        resumed: peaks[2],
    })
}

fn input_block() -> [f32; SAMPLES] {
    std::array::from_fn(|sample| {
        let phase = f32::from(u8::try_from(sample / 2 % 32).expect("phase is below 32")) / 32.0;
        if sample % 2 == 0 {
            0.5 * phase - 0.25
        } else {
            0.375 - 0.5 * phase
        }
    })
}

fn max_difference(left: &[f32], right: &[f32]) -> Result<f32, String> {
    if left.len() != right.len() {
        return Err("PCM comparison lengths differ".into());
    }
    if left.iter().chain(right).any(|sample| !sample.is_finite()) {
        return Err("PCM comparison contains non-finite samples".into());
    }
    Ok(left
        .iter()
        .zip(right)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max))
}

fn parameter_change_evidence(
    before_state: &StateRef,
    after_state: &StateRef,
    before_pcm: &[f32],
    after_pcm: &[f32],
) -> Result<(bool, f32), String> {
    Ok((
        before_state != after_state,
        max_difference(before_pcm, after_pcm)?,
    ))
}

fn remove_effect(
    harness: Harness,
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
) -> Result<(), String> {
    let graph = document(None, None, serde_json::Value::Null);
    let direct_processor = prepare(catalog, controls, &graph)?;
    let exchange = ProcessorExchange::new(1);
    exchange.observe_plan(Arc::clone(direct_processor.plan()));
    let mut direct = Harness {
        processor: direct_processor,
        exchange,
    };
    drop(harness);
    if controls.registry.has_worker(&NodeId::from(EFFECT)) {
        return Err("removed graph still owns the native worker".into());
    }
    let direct_pcm = direct.steady_pcm()?;
    if max_difference(&input_block().repeat(STEADY), &direct_pcm)? > 0.000_1 {
        return Err("effect-free graph did not pass through stereo PCM after removal".into());
    }
    Ok(())
}

fn capture_editor(
    harness: &mut Harness,
    controls: &NativeResources,
    request: &Request,
) -> Result<Option<EditorVerification>, String> {
    if !cfg!(target_os = "macos") {
        return verify_external_editor(harness, controls, request).map(Some);
    }
    let (seconds, path) = request.editor.as_ref().ok_or("editor target missing")?;
    let node = NodeId::from(EFFECT);
    controls.registry.set_editor_open(&node, true)?;
    let deadline = Instant::now() + Duration::from_secs(*seconds);
    let result = wait_editor_frame(harness, controls, &node, deadline).and_then(|frame| {
        let observation = if let Some(seconds) = request.editor_hold {
            Some(observe_native_window_close(
                harness, controls, &node, seconds,
            )?)
        } else {
            request
                .drag
                .map(|points| drag_editor(harness, controls, &frame, points))
                .transpose()?
        };
        let frame = if request.drag.is_some() {
            wait_editor_frame(harness, controls, &node, deadline)?
        } else {
            frame
        };
        Ok((frame, observation))
    });
    let close = controls.registry.set_editor_open(&node, false);
    let (frame, observation) = result?;
    close?;
    let bytes = BASE64
        .decode(frame.png_base64)
        .map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    let close_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < close_deadline {
        let status = controls.registry.editor_status(&node)?;
        if status.state == ProcessorEditorState::Closed {
            println!(
                "editor PNG: {} ({}x{})",
                path.display(),
                frame.width,
                frame.height
            );
            if let Some(observation) = observation {
                let closed_pcm = harness.steady_pcm()?;
                let closed_state = state_reference(&snapshot_effect_state(controls)?)?;
                return observation
                    .verify_closed(&closed_pcm, &closed_state)
                    .map(Some);
            }
            return Ok(None);
        }
        harness.block(&input_block())?;
    }
    Err("native editor did not close after frame capture".into())
}

fn verify_external_editor(
    harness: &mut Harness,
    controls: &NativeResources,
    request: &Request,
) -> Result<EditorVerification, String> {
    let seconds = request
        .editor_hold
        .ok_or("this backend requires --editor-hold and external native window input")?;
    let node = NodeId::from(EFFECT);
    controls.registry.set_editor_open(&node, true)?;
    let result = wait_editor_open(harness, controls, &node)
        .and_then(|()| observe_native_window_close(harness, controls, &node, seconds));
    let close = controls.registry.set_editor_open(&node, false);
    let observation = result?;
    close?;
    let closed_pcm = harness.steady_pcm()?;
    let closed_state = state_reference(&snapshot_effect_state(controls)?)?;
    observation.verify_closed(&closed_pcm, &closed_state)
}

fn wait_editor_open(
    harness: &mut Harness,
    controls: &NativeResources,
    node: &NodeId,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let status = controls.registry.editor_status(node)?;
        match status.state {
            ProcessorEditorState::Open => return Ok(()),
            ProcessorEditorState::Failed | ProcessorEditorState::Unavailable => {
                return Err(format!(
                    "native editor failed to open: {:?}",
                    status.failure
                ));
            }
            ProcessorEditorState::Closed | ProcessorEditorState::Opening => {}
        }
        harness.block(&input_block())?;
    }
    Err("native editor window did not open".into())
}

fn observe_native_window_close(
    harness: &mut Harness,
    controls: &NativeResources,
    node: &NodeId,
    seconds: u64,
) -> Result<DragObservation, String> {
    let before = harness.steady_pcm()?;
    let before_state = state_reference(&snapshot_effect_state(controls)?)?;
    let mut last_open = before.clone();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        match controls.registry.editor_status(node)?.state {
            ProcessorEditorState::Open => last_open = harness.steady_pcm()?,
            ProcessorEditorState::Closed => {
                return Ok(DragObservation {
                    before,
                    pcm: last_open,
                    before_state,
                    edited_state: None,
                });
            }
            ProcessorEditorState::Opening => {}
            ProcessorEditorState::Failed | ProcessorEditorState::Unavailable => {
                return Err("native editor failed during real-window interaction".into());
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err("native editor window was not closed during the bounded hold".into())
}

struct DragObservation {
    before: Vec<f32>,
    pcm: Vec<f32>,
    before_state: StateRef,
    edited_state: Option<StateRef>,
}

struct EditorVerification {
    closed_pcm_difference: f32,
    close_pcm_difference: f32,
    close_state_preservation_verified: bool,
    state_changed: bool,
}

impl DragObservation {
    fn verify_closed(
        &self,
        closed_pcm: &[f32],
        closed_state: &StateRef,
    ) -> Result<EditorVerification, String> {
        let baseline_delta = max_difference(&self.before, closed_pcm)?;
        let close_transition_delta = max_difference(&self.pcm, closed_pcm)?;
        let state_changed = self.before_state != *closed_state;
        if let Some(edited_state) = &self.edited_state {
            if *edited_state == self.before_state {
                return Err("editor drag did not change captured native state".into());
            }
            if edited_state != closed_state {
                return Err("closing the editor changed captured native state".into());
            }
        }
        Ok(EditorVerification {
            closed_pcm_difference: baseline_delta,
            close_pcm_difference: close_transition_delta,
            close_state_preservation_verified: self.edited_state.is_some(),
            state_changed,
        })
    }
}

fn drag_editor(
    harness: &mut Harness,
    controls: &NativeResources,
    frame: &ProcessorEditorFrame,
    [x, y, to_x, to_y]: [f64; 4],
) -> Result<DragObservation, String> {
    if [x, to_x].iter().any(|x| *x >= f64::from(frame.width))
        || [y, to_y].iter().any(|y| *y >= f64::from(frame.height))
    {
        return Err("drag falls outside the actual editor frame".into());
    }
    let node = NodeId::from(EFFECT);
    let before = harness.steady_pcm()?;
    let before_state = state_reference(&snapshot_effect_state(controls)?)?;
    // Queue the complete gesture so a native mouseDown tracking loop always receives mouseUp.
    for (kind, x, y) in [
        (EditorPointerKind::Down, x, y),
        (EditorPointerKind::Move, to_x, to_y),
        (EditorPointerKind::Up, to_x, to_y),
    ] {
        let result = controls.registry.editor_input(
            &node,
            &ProcessorEditorInput {
                instance_id: frame.instance_id.clone(),
                session: frame.session,
                kind,
                x,
                y,
                modifiers: 0,
            },
        );
        if let Err(error) = result {
            let _ = controls.registry.cancel_editor_gesture(&node);
            return Err(error);
        }
    }
    // Frame capture is serviced after the owning main-thread timer consumes pointer input.
    wait_editor_frame(harness, controls, &node, Instant::now() + STATE_TIMEOUT)?;
    let after = harness.steady_pcm()?;
    let edited_state = state_reference(&snapshot_effect_state(controls)?)?;
    Ok(DragObservation {
        before,
        pcm: after,
        before_state,
        edited_state: Some(edited_state),
    })
}

fn wait_editor_frame(
    harness: &mut Harness,
    controls: &NativeResources,
    node: &NodeId,
    deadline: Instant,
) -> Result<mixrelay_control_protocol::ProcessorEditorFrame, String> {
    while Instant::now() < deadline {
        let status = controls.registry.editor_status(node)?;
        match status.state {
            ProcessorEditorState::Open => {
                if let Ok(frame) = controls.registry.editor_frame(node) {
                    return Ok(frame);
                }
            }
            ProcessorEditorState::Failed | ProcessorEditorState::Unavailable => {
                return Err(format!(
                    "native editor failed to open: {:?}",
                    status.failure
                ));
            }
            ProcessorEditorState::Closed | ProcessorEditorState::Opening => {}
        }
        harness.block(&input_block())?;
    }
    Err("native editor frame timed out".into())
}

fn state_reference(state: &serde_json::Value) -> Result<StateRef, String> {
    let value = state
        .get("stateRef")
        .ok_or("native state capture did not return stateRef")?;
    serde_json::from_value(value.clone()).map_err(|error| error.to_string())
}

fn snapshot_effect_state(controls: &NativeResources) -> Result<serde_json::Value, String> {
    controls
        .registry
        .snapshot_state(&NodeId::from(EFFECT), STATE_TIMEOUT)
        .map_err(|error| error.to_string())
}

fn discover_stereo(
    request: &Request,
) -> Result<(ProcessorCatalog, ExternalProcessorDescriptor, ProviderPorts), String> {
    request.target.publish_to_children();
    let catalog = ProcessorCatalog::default();
    let scan_argument = request.target.scan_argument();
    catalog.rescan(std::slice::from_ref(&scan_argument));
    let discovered = catalog.protocol_catalog();
    if !discovered.diagnostics.is_empty() || discovered.processors.len() != 1 {
        return Err(format!(
            "exact target discovery failed: {} processors, diagnostics: {:?}",
            discovered.processors.len(),
            discovered.diagnostics
        ));
    }
    let descriptor = discovered.processors[0].clone();
    let ports = stereo_layout(&descriptor)?;
    if let Some((id, _)) = request.parameter
        && !catalog.parameter_is_writable(&descriptor.type_id, id)
    {
        return Err(format!("parameter {id} is not writable in this effect"));
    }
    Ok((catalog, descriptor, ports))
}

fn read_rescan_step(reader: impl BufRead, expected: &str) -> Result<(), String> {
    let mut line = String::new();
    reader
        .take(64)
        .read_line(&mut line)
        .map_err(|error| format!("rescan input: {error}"))?;
    if !line.ends_with('\n') || line.trim_end_matches(['\r', '\n']) != expected {
        return Err(format!(
            "rescan requires exactly {expected} followed by a newline"
        ));
    }
    Ok(())
}

fn await_rescan_step(expected: &'static str) -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    // A stalled external ACL coordinator must not hold the qualification process indefinitely.
    // On timeout main returns an error and exits; no plug-in worker or catalog lock is held.
    std::thread::spawn(move || {
        let _ = sender.send(read_rescan_step(std::io::stdin().lock(), expected));
    });
    receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|error| format!("rescan {expected} input deadline: {error}"))?
}

fn require_permission_denied(target: &Target) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    let TargetLocation::Bundle(path) = &target.location;
    #[cfg(target_os = "macos")]
    let path = match &target.location {
        TargetLocation::Bundle(path) => path,
        #[cfg(target_os = "macos")]
        TargetLocation::AuComponentId(_) => {
            return Err("permission recovery requires an exact filesystem bundle".into());
        }
    };
    let access = std::fs::metadata(path).and_then(|metadata| {
        if metadata.is_dir() {
            std::fs::read_dir(path).map(|_| ())
        } else {
            std::fs::File::open(path).map(|_| ())
        }
    });
    match access {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Ok(()),
        Err(error) => Err(format!(
            "exact target failed for a non-permission reason: {error}"
        )),
        Ok(()) => {
            Err("exact target remains readable; permission denial was not established".into())
        }
    }
}

fn rescan_recovery(arguments: &[String]) -> Result<(), String> {
    let [format, location] = arguments else {
        return Err("rescan recovery requires one format and one exact bundle".into());
    };
    if format == "au" {
        return Err("rescan recovery does not change Audio Unit registration".into());
    }
    let target = Target::new(format, OsStr::new(location))?;
    target.publish_to_children();
    let catalog = ProcessorCatalog::default();
    let paths = [target.scan_argument()];
    catalog.rescan(&paths);
    let initial = catalog.protocol_catalog();
    if !initial.diagnostics.is_empty() || initial.processors.len() != 1 {
        return Err("initial exact-bundle discovery did not yield one clean effect".into());
    }
    println!(
        "{}",
        serde_json::json!({"phase":"ready-for-denial","catalog":initial})
    );
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    await_rescan_step("denied")?;
    require_permission_denied(&target)?;
    catalog.rescan(&paths);
    let denied = catalog.protocol_catalog();
    if denied.processors != initial.processors
        || denied.diagnostics.is_empty()
        || denied.diagnostics.iter().any(|diagnostic| {
            diagnostic.format != initial.processors[0].format
                || diagnostic.stage != ProcessorDiscoveryStage::Scan
                || diagnostic.message.is_empty()
        })
    {
        return Err(format!(
            "denied rescan lost its usable catalog or scan cause: {denied:?}"
        ));
    }
    println!(
        "{}",
        serde_json::json!({"phase":"denial-preserved","permissionDeniedRead":true,"catalog":denied})
    );
    std::io::stdout()
        .flush()
        .map_err(|error| error.to_string())?;
    await_rescan_step("restored")?;
    catalog.rescan(&paths);
    let restored = catalog.protocol_catalog();
    if restored.processors != initial.processors || !restored.diagnostics.is_empty() {
        return Err(format!(
            "restored rescan did not recover the same clean catalog: {restored:?}"
        ));
    }
    println!(
        "{}",
        serde_json::json!({"phase":"complete","sameCatalogRetained":true,"format":format,"target":paths[0],"initial":initial,"denied":denied,"restored":restored,"physicalPcmQualified":false,"realTimeQualified":false})
    );
    Ok(())
}

fn wide_layout(layouts: &[ProviderPorts]) -> Result<ProviderPorts, String> {
    layouts
        .iter()
        .find(|ports| {
            ports.total_input_channels() == Ok(64)
                && ports.total_output_channels() == Ok(64)
                && ports.inputs.len() == 1
                && ports.outputs.len() == 1
        })
        .cloned()
        .ok_or("requested AU has no accepted single-bus 64-in/64-out layout".into())
}

fn wide_document(
    type_id: Option<&str>,
    ports: Option<&ProviderPorts>,
    state: serde_json::Value,
) -> GraphDocument {
    let mut graph = document(type_id, ports, state);
    for node in &mut graph.nodes {
        match &mut node.config {
            NodeConfig::AudioDeviceInput(input) => input.output_channels = 64,
            NodeConfig::AudioDeviceOutput(output) => output.input_channels = 16,
            _ => {}
        }
    }
    let source = if type_id.is_some() { EFFECT } else { "in" };
    for edge in &mut graph.edges {
        if edge.destination.node_id.as_str() == "out" {
            edge.channel_mapping = ChannelMapping::Explicit {
                routes: (0..16)
                    .map(|channel| ChannelRoute {
                        source_channel: channel + 48,
                        destination_channel: channel,
                    })
                    .collect(),
            };
        }
    }
    graph.nodes.push(node(
        "reference",
        NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
            selector: DeviceSelector::DefaultOutput,
            input_channels: 64,
        }),
    ));
    graph.edges.push(edge("reference", source, "reference"));
    graph
}

fn timecoded_sample(frame: u32, channel: u16) -> f32 {
    let phase = u16::try_from((frame % 251 + u32::from(channel) * 17) % 251)
        .expect("timecode phase is below 251");
    (f32::from(phase) - 125.0) / 4096.0 + f32::from(channel) / 1024.0
}

fn wide_mapping_difference(mapped: &[f32], reference: &[f32]) -> Result<f32, String> {
    if reference.is_empty()
        || !reference.len().is_multiple_of(64)
        || mapped.len() != reference.len() / 4
    {
        return Err("64-to-16 PCM geometry differs from the selected route".into());
    }
    pcm_peak(reference)?;
    let mut difference = 0.0_f32;
    // Both sinks read the same AU output in the same block. Its native latency and DSP are
    // common to both branches; this oracle makes no identity-DSP or input/output delay claim.
    for (selected, full) in mapped.chunks_exact(16).zip(reference.chunks_exact(64)) {
        difference = difference.max(max_difference(selected, &full[48..64])?);
    }
    Ok(difference)
}

struct WideHarness {
    processor: Box<BlockProcessor>,
    exchange: ProcessorExchange,
    format: RenderFormat,
    input: Vec<f32>,
    mapped: Vec<f32>,
    reference: Vec<f32>,
    mapped_first: bool,
    direct: bool,
    frame: u32,
}

impl WideHarness {
    fn new(
        catalog: &ProcessorCatalog,
        controls: &NativeResources,
        graph: &GraphDocument,
        format: RenderFormat,
    ) -> Result<Self, String> {
        let processor = prepare_at_format(catalog, controls, graph, format)?;
        let sources = processor.plan().source_bindings();
        let sinks = processor.plan().sink_bindings();
        if sources.len() != 1 || sources[0].channels() != 64 || sinks.len() != 2 {
            return Err("wide graph requires one 64-channel source and two sinks".into());
        }
        let mapped_first = sinks[0].node_id().as_str() == "out";
        let (mapped, reference) = if mapped_first {
            (&sinks[0], &sinks[1])
        } else {
            (&sinks[1], &sinks[0])
        };
        if mapped.node_id().as_str() != "out"
            || mapped.channels() != 16
            || reference.node_id().as_str() != "reference"
            || reference.channels() != 64
        {
            return Err("wide graph sink identities or channel widths differ".into());
        }
        let exchange = ProcessorExchange::new(1);
        exchange.observe_plan(Arc::clone(processor.plan()));
        let frames = usize::try_from(format.block_frames).map_err(|error| error.to_string())?;
        Ok(Self {
            processor,
            exchange,
            format,
            input: vec![0.0; frames * 64],
            mapped: vec![0.0; frames * 16],
            reference: vec![0.0; frames * 64],
            mapped_first,
            direct: !graph
                .nodes
                .iter()
                .any(|node| matches!(node.config, NodeConfig::Provider(_))),
            frame: 0,
        })
    }

    fn block(&mut self) -> Result<(), String> {
        for (frame, samples) in self.input.chunks_exact_mut(64).enumerate() {
            let position = self.frame + u32::try_from(frame).map_err(|error| error.to_string())?;
            for (channel, sample) in samples.iter_mut().enumerate() {
                *sample = timecoded_sample(
                    position,
                    u16::try_from(channel).map_err(|error| error.to_string())?,
                );
            }
        }
        self.exchange.refresh_latency()?;
        self.exchange.activate_pending(&mut self.processor);
        let mut outputs = if self.mapped_first {
            [self.mapped.as_mut_slice(), self.reference.as_mut_slice()]
        } else {
            [self.reference.as_mut_slice(), self.mapped.as_mut_slice()]
        };
        self.processor
            .process_block(self.format.block_frames, &[&self.input], &mut outputs)
            .map_err(|error| error.to_string())?;
        if wide_mapping_difference(&self.mapped, &self.reference)? > 0.000_1 {
            return Err("explicit 48..63-to-0..15 mapping differs from the same AU output".into());
        }
        if self.direct && max_difference(&self.input, &self.reference)? > 0.000_1 {
            return Err("effect-free graph changed timecoded PCM".into());
        }
        self.frame = self
            .frame
            .checked_add(self.format.block_frames)
            .ok_or("timecode position overflow")?;
        std::thread::sleep(Duration::from_millis(10));
        self.exchange.reclaim_retired();
        Ok(())
    }

    fn steady_pcm(&mut self) -> Result<Vec<f32>, String> {
        self.frame = 0;
        let mut result = Vec::with_capacity(self.mapped.len() * STEADY);
        for _ in 0..WARMUP {
            self.block()?;
        }
        for _ in 0..STEADY {
            self.block()?;
            result.extend_from_slice(&self.mapped);
        }
        pcm_peak(&result)?;
        Ok(result)
    }

    fn signal_gates(&mut self, controls: &NativeResources) -> Result<SignalGateEvidence, String> {
        let plan = Arc::clone(self.processor.plan());
        let mut peaks = [0.0; 3];
        for ((revision, control), peak) in [
            (
                2,
                NodeControl {
                    enabled: false,
                    muted: false,
                },
            ),
            (
                3,
                NodeControl {
                    enabled: true,
                    muted: true,
                },
            ),
            (4, NodeControl::default()),
        ]
        .into_iter()
        .zip(&mut peaks)
        {
            self.exchange
                .publish_controls(
                    self.processor.plan().controls(),
                    revision,
                    &[RuntimeControlUpdate {
                        node_id: NodeId::from(EFFECT),
                        control,
                        gain_db: None,
                    }],
                )
                .map_err(|error| error.to_string())?;
            *peak = pcm_peak(&self.steady_pcm()?)?;
            if self.exchange.revisions().active != revision
                || !Arc::ptr_eq(&plan, self.processor.plan())
                || !controls.registry.has_worker(&NodeId::from(EFFECT))
            {
                return Err(
                    "wide signal gate changed native ownership or missed its revision".into(),
                );
            }
            if (!control.enabled || control.muted) && *peak != 0.0 {
                return Err("wide OFF or mute passed nonzero PCM".into());
            }
        }
        if peaks[2] <= 0.000_01 {
            return Err("wide AU failed to resume nonzero PCM".into());
        }
        Ok(SignalGateEvidence {
            off: peaks[0],
            muted: peaks[1],
            resumed: peaks[2],
        })
    }
}

fn remove_wide(
    harness: WideHarness,
    catalog: &ProcessorCatalog,
    controls: &NativeResources,
) -> Result<(), String> {
    let format = harness.format;
    drop(harness);
    if controls.registry.has_worker(&NodeId::from(EFFECT)) {
        return Err("removed wide AU still owns its native worker".into());
    }
    let graph = wide_document(None, None, serde_json::Value::Null);
    WideHarness::new(catalog, controls, &graph, format)?.steady_pcm()?;
    Ok(())
}

fn run_wide_au(arguments: &[String]) -> Result<(), String> {
    let [component] = arguments else {
        return Err("--au-64-to-16 needs one exact AU component ID".into());
    };
    let request = Request {
        target: Target::new("au", OsStr::new(component))?,
        parameter: None,
        editor: None,
        drag: None,
        editor_hold: None,
    };
    let (catalog, descriptor, _) = discover_stereo(&request)?;
    let ports = wide_layout(&descriptor.layouts)?;
    let artifacts = mixrelay_test_support::FixtureDir::new("native-au-wide")
        .map_err(|error| error.to_string())?;
    let controls = NativeResources::new(Arc::new(mixrelay_time::StdClock::new()));
    controls
        .files
        .select(mixrelay_processor_state::StateStore::new(
            artifacts.join("states"),
        ));
    let graph = wide_document(
        Some(&descriptor.type_id),
        Some(&ports),
        serde_json::json!({}),
    );
    let mut first = WideHarness::new(&catalog, &controls, &graph, FORMAT)?;
    let gates = first.signal_gates(&controls)?;
    let edited_parameter = descriptor
        .parameters
        .iter()
        .find(|parameter| !parameter.is_read_only);
    if let Some(parameter) = edited_parameter {
        controls
            .registry
            .set_parameter(&NodeId::from(EFFECT), parameter.id, 0.37)?;
        first.steady_pcm()?;
    }
    let state = snapshot_effect_state(&controls)?;
    let saved = state_reference(&state)?;
    let baseline = first.steady_pcm()?;
    remove_wide(first, &catalog, &controls)?;
    let graph = wide_document(Some(&descriptor.type_id), Some(&ports), state);
    let alternate = RenderFormat {
        sample_rate: 44_100,
        block_frames: 256,
    };
    let mut changed = WideHarness::new(&catalog, &controls, &graph, alternate)?;
    let alternate_peak = pcm_peak(&changed.steady_pcm()?)?;
    let alternate_state = state_reference(&snapshot_effect_state(&controls)?)?;
    remove_wide(changed, &catalog, &controls)?;
    let mut restored = WideHarness::new(&catalog, &controls, &graph, FORMAT)?;
    let recovered = restored.steady_pcm()?;
    let difference = max_difference(&baseline, &recovered)?;
    let restored_state = state_reference(&snapshot_effect_state(&controls)?)?;
    if restored_state != saved || difference > 0.001 {
        return Err(format!(
            "same-format AU state/PCM restoration differs: {difference}"
        ));
    }
    remove_wide(restored, &catalog, &controls)?;
    println!(
        "{}",
        serde_json::json!({
            "format":"au", "target":component, "layoutId":ports.layout_id,
            "inputChannels":64, "outputChannels":16, "referenceChannels":64,
            "mapping":"48..63 -> 0..15", "mappingOracle":"same AU output, same block",
            "timecodedPcm":true, "formats":[FORMAT,alternate,FORMAT],
            "formatTransition":"saved-state fresh-worker reprepare", "semanticParameterReadbackVerified":false,
            "requestedParameter":edited_parameter.map(|parameter| serde_json::json!({"id":parameter.id,"value":0.37})),
            "alternateFormatPeak":alternate_peak, "alternateState":{"bytes":alternate_state.byte_length(),"sha256":alternate_state.sha256()},
            "savedState":{"bytes":saved.byte_length(),"sha256":saved.sha256()},
            "restoredState":{"bytes":restored_state.byte_length(),"sha256":restored_state.sha256()},
            "restoredStateExact":true, "restoredPcmMaxDifference":difference,
            "signalGates":{"offPeak":gates.off,"mutePeak":gates.muted,"resumedPeak":gates.resumed},
            "workerRegistry":"empty", "editorAdvertised":descriptor.has_editor, "editorEvidence":"none",
            "physicalPcmQualified":false, "realTimeQualified":false
        })
    );
    Ok(())
}

struct QualificationEvidence {
    default_reference: StateRef,
    saved_reference: StateRef,
    restored_reference: StateRef,
    parameter_pcm_difference: Option<f32>,
    parameter_state_changed: Option<bool>,
    restore_difference: f32,
    editor_verification: Option<EditorVerification>,
    signal_gates: SignalGateEvidence,
}

fn run(request: &Request) -> Result<(), String> {
    let (catalog, descriptor, ports) = discover_stereo(request)?;
    let artifacts = mixrelay_test_support::FixtureDir::new("native-vendor-qualification")
        .map_err(|error| error.to_string())?;
    let controls = NativeResources::new(Arc::new(mixrelay_time::StdClock::new()));
    controls
        .files
        .select(mixrelay_processor_state::StateStore::new(
            artifacts.join("states"),
        ));
    let mut first = Harness::new(
        &catalog,
        &controls,
        &descriptor,
        &ports,
        serde_json::json!({}),
    )
    .map_err(|error| format!("initial worker preparation: {error}"))?;
    let default_pcm = first.steady_pcm()?;
    if !default_pcm.iter().any(|sample| sample.abs() > 0.000_01) {
        return Err("default native effect produced no nonzero steady PCM".into());
    }
    // Exercise controls before parameter edits: a valid later edit may intentionally silence
    // the plug-in. This verifies PCM gating and topology/worker preservation, not DSP CPU cost.
    let signal_gates = qualify_signal_gates(&mut first, &controls)?;
    let default_state = snapshot_effect_state(&controls)?;
    let default_reference = state_reference(&default_state)?;
    let mut state = default_state.clone();
    let mut edit_difference = None;
    let mut parameter_state_changed = None;
    if let Some((id, value)) = request.parameter {
        controls
            .registry
            .set_parameter(&NodeId::from(EFFECT), id, value)?;
        let edited_pcm = first.steady_pcm()?;
        state = snapshot_effect_state(&controls)?;
        let (state_changed, difference) = parameter_change_evidence(
            &default_reference,
            &state_reference(&state)?,
            &default_pcm,
            &edited_pcm,
        )?;
        edit_difference = Some(difference);
        parameter_state_changed = Some(state_changed);
    }
    let editor_verification = if request.editor.is_some() || request.editor_hold.is_some() {
        let difference = capture_editor(&mut first, &controls, request)?;
        state = snapshot_effect_state(&controls)?;
        difference
    } else {
        None
    };
    let saved_reference = state_reference(&state)?;
    let baseline = first.steady_pcm()?;
    remove_effect(first, &catalog, &controls)?;
    let mut restored = Harness::new(&catalog, &controls, &descriptor, &ports, state)
        .map_err(|error| format!("fresh worker state restoration: {error}"))?;
    let restored_state = snapshot_effect_state(&controls)?;
    let restored_reference = state_reference(&restored_state)?;
    let recovered = restored.steady_pcm()?;
    let restore_difference = max_difference(&baseline, &recovered)?;
    if restore_difference > 0.001 {
        return Err(format!(
            "fresh worker PCM differs from captured state: {restore_difference}"
        ));
    }
    remove_effect(restored, &catalog, &controls)?;
    report_qualification(
        request,
        &descriptor,
        &ports,
        &QualificationEvidence {
            default_reference,
            saved_reference,
            restored_reference,
            parameter_pcm_difference: edit_difference,
            parameter_state_changed,
            restore_difference,
            editor_verification,
            signal_gates,
        },
    );
    Ok(())
}

fn report_qualification(
    request: &Request,
    descriptor: &ExternalProcessorDescriptor,
    ports: &ProviderPorts,
    evidence: &QualificationEvidence,
) {
    println!(
        "{}",
        serde_json::json!({
            "format": request.target.format.argument(),
            "target": request.target.request_argument().to_string_lossy(),
            "typeId": descriptor.type_id, "layoutId": ports.layout_id,
            "sampleRate": FORMAT.sample_rate, "blockFrames": FRAMES,
            "stereoPcm": "baseline finite nonzero; later finite", "parameterPcmMaxDifference": evidence.parameter_pcm_difference,
            "parameterSerializedStateChanged": evidence.parameter_state_changed,
            "restoredPcmMaxDifference": evidence.restore_difference, "workerRegistry": "empty",
            "signalGates": {
                "offPeak": evidence.signal_gates.off,
                "mutePeak": evidence.signal_gates.muted,
                "resumedPeak": evidence.signal_gates.resumed,
                "planAndWorkerPreserved": true,
                "dspCpuCostQualified": false
            },
            "defaultState": {"bytes": evidence.default_reference.byte_length(), "sha256": evidence.default_reference.sha256()},
            "savedState": {"bytes": evidence.saved_reference.byte_length(), "sha256": evidence.saved_reference.sha256()},
            "restoredState": {"bytes": evidence.restored_reference.byte_length(), "sha256": evidence.restored_reference.sha256()},
            "restoredStateExact": evidence.saved_reference == evidence.restored_reference,
            "editorPng": request.editor.as_ref().filter(|_| cfg!(target_os = "macos")).map(|(_, path)| path),
            "editorEvidence": if request.editor_hold.is_some() {
                "external-window-input"
            } else if request.drag.is_some() {
                "queued-editor-drag"
            } else if request.editor.is_some() {
                "frame-and-close-only"
            } else {
                "none"
            },
            "editorInteractionPcmMaxDifference": evidence.editor_verification.as_ref().map(|result| result.closed_pcm_difference),
            "editorClosePcmMaxDifference": evidence.editor_verification.as_ref().map(|result| result.close_pcm_difference),
            "editorCloseStatePreservationVerified": evidence.editor_verification.as_ref().map(|result| result.close_state_preservation_verified),
            "editorSerializedStateChanged": evidence.editor_verification.as_ref().map(|result| result.state_changed),
            "physicalPcmQualified": false, "realTimeQualified": false
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn wide_profile_selects_only_an_advertised_exact_layout() {
        use mixrelay_audio_graph::{ProviderBus, ProviderBusKind, ProviderChannelFormat};
        let bus = ProviderBus {
            id: AUDIO_PORT_ID.into(),
            name: "Main".into(),
            kind: ProviderBusKind::Main,
            channels: 64,
            channel_format: ProviderChannelFormat::Discrete,
        };
        let accepted = ProviderPorts {
            layout_id: "accepted-64".into(),
            inputs: vec![bus.clone()],
            outputs: vec![bus],
        };
        let mut rejected = accepted.clone();
        rejected.outputs[0].channels = 16;
        assert!(wide_layout(&[rejected.clone()]).is_err());
        assert_eq!(
            wide_layout(&[rejected, accepted.clone()]).unwrap(),
            accepted
        );
        assert!(wide_layout(&[]).is_err());
        assert!(run_wide_au(&[]).is_err());
        assert!(run_wide_au(&arguments(&["bad-component-id"])).is_err());
    }

    #[test]
    fn wide_mapping_uses_the_same_post_dsp_channels_and_rejects_invalid_pcm() {
        let mut reference: Vec<_> = (0..2)
            .flat_map(|frame| (0..64).map(move |channel| timecoded_sample(frame, channel)))
            .collect();
        let mapped: Vec<_> = reference
            .chunks_exact(64)
            .flat_map(|frame| frame[48..64].iter().copied())
            .collect();
        assert_eq!(wide_mapping_difference(&mapped, &reference).unwrap(), 0.0);
        let wrong: Vec<_> = reference
            .chunks_exact(64)
            .flat_map(|frame| frame[..16].iter().copied())
            .collect();
        assert!(wide_mapping_difference(&wrong, &reference).unwrap() > 0.001);
        assert!(wide_mapping_difference(&mapped[..31], &reference).is_err());
        reference[0] = f32::NAN;
        assert!(wide_mapping_difference(&mapped, &reference).is_err());
    }

    #[test]
    fn timecoded_samples_distinguish_channels_and_adjacent_frames() {
        for frame in [0, 250, 251, 4095] {
            let values: Vec<_> = (0..64)
                .map(|channel| timecoded_sample(frame, channel))
                .collect();
            assert!(
                values
                    .iter()
                    .all(|value| value.is_finite() && value.abs() < 0.1)
            );
            for (channel, value) in values.iter().enumerate() {
                assert!(!values[..channel].contains(value));
                assert_ne!(
                    *value,
                    timecoded_sample(frame + 1, u16::try_from(channel).unwrap())
                );
            }
        }
    }

    #[test]
    fn effect_free_wide_graph_routes_timecoded_pcm_at_both_requested_formats() {
        let catalog = ProcessorCatalog::default();
        let controls = NativeResources::new(Arc::new(mixrelay_time::ManualClock::default()));
        let graph = wide_document(None, None, serde_json::Value::Null);
        assert_eq!(graph_input_width(&graph), 64);
        for format in [
            FORMAT,
            RenderFormat {
                sample_rate: 44_100,
                block_frames: 256,
            },
        ] {
            let mut harness = WideHarness::new(&catalog, &controls, &graph, format).unwrap();
            let first = harness.steady_pcm().unwrap();
            let repeated = harness.steady_pcm().unwrap();
            assert_eq!(
                first.len(),
                usize::try_from(format.block_frames).unwrap() * 16 * STEADY
            );
            assert_eq!(first, repeated);
            assert!(pcm_peak(&first).unwrap() > 0.000_01);
            remove_wide(harness, &catalog, &controls).unwrap();
        }
        assert!(!controls.registry.has_worker(&NodeId::from(EFFECT)));
    }

    #[test]
    fn signal_gate_peak_rejects_missing_and_non_finite_pcm() {
        assert_eq!(pcm_peak(&[0.0, -0.0]).unwrap(), 0.0);
        assert_eq!(pcm_peak(&[-0.5, 0.25]).unwrap(), 0.5);
        assert!(pcm_peak(&[]).is_err());
        assert!(pcm_peak(&[f32::NAN]).is_err());
        assert!(pcm_peak(&[f32::INFINITY]).is_err());
    }

    #[test]
    fn run_requires_explicit_format_and_exact_bundle_extension() {
        assert!(parse_request(&[]).is_err());
        assert!(parse_request(&arguments(&["--run", "au", "/missing.vst3"])).is_err());
        let owned = mixrelay_test_support::FixtureDir::new("vendor-qualification-cli").unwrap();
        let bundle = owned.join("plugin.clap");
        std::fs::create_dir(&bundle).unwrap();
        let path = bundle.to_str().unwrap();
        assert!(parse_request(&arguments(&["--run", "vst3", path])).is_err());
        assert!(parse_request(&arguments(&["--run", "clap", path])).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn au_target_requires_three_exact_fourcc_fields() {
        let target = Target::new("au", OsStr::new("aufx:lmtr:appl")).unwrap();
        assert_eq!(target.format, RackFormat::Au);
        assert_eq!(target.request_argument(), OsStr::new("aufx:lmtr:appl"));
        for malformed in [
            "aufx:lmtr",
            "aufx:lmtr:appl:extra",
            "au:lmtr:appl",
            "aufx:longg:appl",
            "aufx:lmtr:äpp1",
            "aumf:lmtr:applx",
        ] {
            assert!(
                Target::new("au", OsStr::new(malformed)).is_err(),
                "{malformed}"
            );
        }
        assert!(Target::new("au", OsStr::new("aumu:lmtr:appl")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn au_locator_must_match_exact_component_and_empty_path() {
        fn locator(unique_id: &str, path: &str, format: &str) -> RackLocator {
            let value = serde_json::json!({
                "format": format,
                "uniqueId": unique_id,
                "path": path,
                "name": "Qualification fixture",
                "vendor": "Fixture",
                "version": 1,
                "category": "effect",
                "hasEditor": false,
                "acceptsMidi": false
            });
            let encoded = BASE64.encode(serde_json::to_vec(&value).unwrap());
            RackLocator::decode(&encoded).unwrap()
        }

        let target = Target::new("au", OsStr::new("aufx:lmtr:appl")).unwrap();
        assert!(
            target
                .require_locator(&locator("aufx:lmtr:appl", "", "au"))
                .is_ok()
        );
        assert!(
            target
                .require_locator(&locator("aufx:other:appl", "", "au"))
                .is_err()
        );
        assert!(
            target
                .require_locator(&locator("aufx:lmtr:appl", "/not-an-au-bundle", "au"))
                .is_err()
        );
        assert!(
            target
                .require_locator(&locator("aufx:lmtr:appl", "", "auv3"))
                .is_err()
        );
        assert!(
            target
                .require_scan_argument(OsStr::new("aufx:other:appl"))
                .is_err()
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn au_is_unavailable_on_non_macos_targets() {
        assert!(Target::new("au", OsStr::new("aufx:lmtr:appl")).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_lv2_requires_an_exact_bundle_extension() {
        let owned = mixrelay_test_support::FixtureDir::new("vendor-qualification-lv2-cli").unwrap();
        let bundle = owned.join("plugin.lv2");
        std::fs::create_dir(&bundle).unwrap();
        let path = bundle.to_str().unwrap();
        assert!(parse_request(&arguments(&["--run", "lv2", path])).is_ok());
        assert!(parse_request(&arguments(&["--run", "clap", path])).is_err());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn lv2_is_not_a_qualification_target_on_other_platforms() {
        let owned = mixrelay_test_support::FixtureDir::new("vendor-qualification-lv2-cli").unwrap();
        let bundle = owned.join("plugin.lv2");
        std::fs::create_dir(&bundle).unwrap();
        assert!(parse_request(&arguments(&["--run", "lv2", bundle.to_str().unwrap()])).is_err());
    }

    #[test]
    fn run_rejects_invalid_parameter_values_and_duplicate_options() {
        let owned = mixrelay_test_support::FixtureDir::new("vendor-qualification-options").unwrap();
        let bundle = owned.join("plugin.vst3");
        std::fs::create_dir(&bundle).unwrap();
        let path = bundle.to_str().unwrap();
        for value in ["-0.1", "1.1", "NaN"] {
            assert!(
                parse_request(&arguments(&[
                    "--run",
                    "vst3",
                    path,
                    "--parameter",
                    "7",
                    value,
                ]))
                .is_err()
            );
        }
        assert!(
            parse_request(&arguments(&[
                "--run",
                "vst3",
                path,
                "--parameter",
                "7",
                "0.4",
                "--parameter",
                "8",
                "0.5",
            ]))
            .is_err()
        );
    }

    #[test]
    fn editor_drag_requires_a_frame_and_finite_bounded_coordinates() {
        let owned = mixrelay_test_support::FixtureDir::new("vendor-editor-drag-cli").unwrap();
        let bundle = owned.join("plugin.clap");
        std::fs::create_dir(&bundle).unwrap();
        let prefix = ["--run", "clap", bundle.to_str().unwrap()];
        let png = owned.join("editor.png");
        let no_frame = [prefix.as_slice(), &["--drag", "1", "2", "3", "4"]].concat();
        assert!(parse_request(&arguments(&no_frame)).is_err());
        for x in ["NaN", "inf", "-1", "8193"] {
            let values = [
                prefix.as_slice(),
                &[
                    "--editor",
                    "10",
                    "/tmp/editor.png",
                    "--drag",
                    x,
                    "2",
                    "3",
                    "4",
                ],
            ]
            .concat();
            assert!(parse_request(&arguments(&values)).is_err());
        }
        let valid = [
            prefix.as_slice(),
            &[
                "--editor",
                "10",
                png.to_str().unwrap(),
                "--drag",
                "1",
                "2",
                "3",
                "4",
            ],
        ]
        .concat();
        let parsed = parse_request(&arguments(&valid));
        if cfg!(target_os = "macos") {
            assert_eq!(parsed.unwrap().drag, Some([1.0, 2.0, 3.0, 4.0]));
        } else {
            assert!(parsed.is_err());
        }
    }

    #[test]
    fn editor_hold_requires_a_frame_only_on_macos_and_excludes_queued_input() {
        let owned = mixrelay_test_support::FixtureDir::new("vendor-editor-hold-cli").unwrap();
        let bundle = owned.join("plugin.clap");
        std::fs::create_dir(&bundle).unwrap();
        let path = bundle.to_str().unwrap();
        let prefix = ["--run", "clap", path];
        let png = owned.join("editor.png");
        let standalone = parse_request(&arguments(
            &[prefix.as_slice(), &["--editor-hold", "5"]].concat(),
        ));
        assert_eq!(standalone.is_err(), cfg!(target_os = "macos"));
        if !cfg!(target_os = "macos") {
            let request = standalone.unwrap();
            assert!(request.editor.is_none());
            assert_eq!(request.editor_hold, Some(5));
        }
        for seconds in ["0", "31", "NaN"] {
            assert!(
                parse_request(&arguments(
                    &[prefix.as_slice(), &["--editor-hold", seconds]].concat()
                ))
                .is_err()
            );
            assert!(
                parse_request(&arguments(
                    &[
                        prefix.as_slice(),
                        &[
                            "--editor",
                            "10",
                            "/tmp/editor.png",
                            "--editor-hold",
                            seconds
                        ]
                    ]
                    .concat()
                ))
                .is_err()
            );
        }
        let valid = [
            prefix.as_slice(),
            &[
                "--editor",
                "10",
                png.to_str().unwrap(),
                "--editor-hold",
                "5",
            ],
        ]
        .concat();
        let parsed = parse_request(&arguments(&valid));
        if cfg!(target_os = "macos") {
            assert_eq!(parsed.unwrap().editor_hold, Some(5));
        } else {
            assert!(parsed.is_err());
        }
        let mixed = [valid.as_slice(), &["--drag", "1", "2", "3", "4"]].concat();
        assert!(parse_request(&arguments(&mixed)).is_err());
    }

    #[test]
    fn pcm_difference_rejects_length_and_nonfinite_samples() {
        assert!(max_difference(&[0.0], &[0.0, 1.0]).is_err());
        assert!(max_difference(&[f32::NAN], &[0.0]).is_err());
        assert!(max_difference(&[0.0], &[f32::INFINITY]).is_err());
        assert_eq!(max_difference(&[0.25, -0.25], &[0.0, 0.25]).unwrap(), 0.5);
    }

    #[test]
    fn rescan_steps_require_exact_bounded_phase_acknowledgments() {
        for input in [b"denied\n".as_slice(), b"denied\r\n".as_slice()] {
            assert!(read_rescan_step(std::io::Cursor::new(input), "denied").is_ok());
        }
        assert!(read_rescan_step(std::io::Cursor::new(b"restored\n"), "restored").is_ok());
        for input in ["", "denied", "restored\n", " denied\n", "denied extra\n"] {
            assert!(read_rescan_step(std::io::Cursor::new(input), "denied").is_err());
        }
        assert!(read_rescan_step(std::io::Cursor::new("x".repeat(128)), "denied").is_err());
    }

    #[test]
    fn permission_recovery_rejects_a_readable_or_missing_bundle() {
        let owned =
            mixrelay_test_support::FixtureDir::new("discovery-permission-boundary").unwrap();
        let bundle = owned.join("effect.clap");
        std::fs::write(&bundle, b"not a plugin").unwrap();
        let target = Target::new("clap", bundle.as_os_str()).unwrap();
        assert!(
            require_permission_denied(&target)
                .unwrap_err()
                .contains("remains readable")
        );
        std::fs::remove_file(bundle).unwrap();
        assert!(
            require_permission_denied(&target)
                .unwrap_err()
                .contains("non-permission")
        );
    }

    #[test]
    fn parameter_state_change_can_have_unchanged_or_silent_steady_pcm() {
        let before = StateRef::new("a".repeat(64), 1).unwrap();
        let after = StateRef::new("b".repeat(64), 1).unwrap();
        assert_eq!(
            parameter_change_evidence(&before, &after, &[0.25], &[0.25]).unwrap(),
            (true, 0.0)
        );
        assert_eq!(
            parameter_change_evidence(&before, &after, &[0.25], &[0.0]).unwrap(),
            (true, 0.25)
        );
        assert_eq!(
            parameter_change_evidence(&before, &before, &[0.25], &[0.25]).unwrap(),
            (false, 0.0)
        );
        assert!(parameter_change_evidence(&before, &after, &[0.25], &[]).is_err());
        assert!(parameter_change_evidence(&before, &after, &[0.25], &[f32::NAN]).is_err());
    }

    #[test]
    fn effect_free_graph_passes_stereo_pcm() {
        let catalog = ProcessorCatalog::default();
        let controls = NativeResources::new(Arc::new(mixrelay_time::ManualClock::default()));
        let graph = document(None, None, serde_json::Value::Null);
        let processor = prepare(&catalog, &controls, &graph).unwrap();
        let exchange = ProcessorExchange::new(1);
        exchange.observe_plan(Arc::clone(processor.plan()));
        let mut harness = Harness {
            processor,
            exchange,
        };
        let output = harness.steady_pcm().unwrap();
        assert!(max_difference(&input_block().repeat(STEADY), &output).unwrap() <= 0.000_1);
    }

    #[test]
    fn editor_drag_uses_native_state_and_close_only_accepts_unchanged_pcm() {
        let before_state = StateRef::new("a".repeat(64), 1).unwrap();
        let edited_state = StateRef::new("b".repeat(64), 1).unwrap();
        let observation = DragObservation {
            before: vec![0.0],
            pcm: vec![0.000_2],
            before_state: before_state.clone(),
            edited_state: Some(edited_state.clone()),
        };
        let verified = observation
            .verify_closed(&[0.000_2], &edited_state)
            .unwrap();
        assert_eq!(verified.closed_pcm_difference, 0.000_2);
        assert!(verified.close_state_preservation_verified);
        assert!(verified.state_changed);
        // A valid native-state edit need not affect this input's PCM.
        assert!(observation.verify_closed(&[0.0], &edited_state).is_ok());
        assert!(observation.verify_closed(&[0.002], &edited_state).is_ok());
        assert!(observation.verify_closed(&[0.0], &before_state).is_err());
        assert!(
            observation
                .verify_closed(&[f32::NAN], &edited_state)
                .is_err()
        );
        let unchanged = DragObservation {
            before: vec![0.0],
            pcm: vec![0.0],
            before_state: before_state.clone(),
            edited_state: None,
        };
        let close_only = unchanged.verify_closed(&[0.0], &before_state).unwrap();
        assert!(!close_only.state_changed);
        assert!(!close_only.close_state_preservation_verified);
        let late_change = unchanged.verify_closed(&[0.000_2], &edited_state).unwrap();
        assert_eq!(late_change.closed_pcm_difference, 0.000_2);
        assert!(!late_change.close_state_preservation_verified);
        let ineffective_drag = DragObservation {
            edited_state: Some(before_state.clone()),
            ..unchanged
        };
        assert!(
            ineffective_drag
                .verify_closed(&[0.0], &before_state)
                .is_err()
        );
    }
}
