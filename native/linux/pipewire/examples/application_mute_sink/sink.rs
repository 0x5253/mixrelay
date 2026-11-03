use super::{
    callback_sink::{CallbackSink, PhaseState},
    trace::TraceConfig,
};
use pipewire::{
    context::ContextRc,
    link::{Link, LinkListener, LinkState},
    loop_::{Signal, SignalSource, Timeout},
    main_loop::MainLoopRc,
    node::{Node, NodeChangeMask, NodeInfoRef, NodeListener, NodeState},
    properties::properties,
    registry::Listener as RegistryListener,
    spa::{
        param::{
            ParamType,
            audio::{AudioFormat, AudioInfoRaw},
        },
        pod::{Object, Pod, Value, serialize::PodSerializer},
        utils::Direction,
    },
    stream::{Stream, StreamBox, StreamFlags, StreamState},
    types::ObjectType,
};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    io::{BufRead, Cursor, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::Ordering,
        mpsc::{Receiver, TryRecvError, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) use super::trace::SinkError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const SETUP_TIMEOUT: Duration = Duration::from_secs(3);
const ROUTE_TIMEOUT: Duration = Duration::from_secs(10);
const GRAPH_OUTPUT_NODE_NAME: &str = "io.mixrelay.engine-output.device-output";
const MAX_OBSERVED_INCOMING_LINKS: usize = 4;
const OBSERVER_STREAM_FLAGS: StreamFlags = StreamFlags::MAP_BUFFERS.union(StreamFlags::RT_PROCESS);

struct GraphRouteExpectation {
    process_id: u32,
    start_time: u64,
    node_name: Rc<str>,
}

enum ObserverMode {
    DirectSource { node_id: u32, process_id: u32 },
    GraphRoute { route: GraphRouteExpectation },
    PolicySink,
}

impl ObserverMode {
    fn parse(arguments: &[String]) -> Result<Self, SinkError> {
        match arguments {
            [mode] if mode == "--policy-sink" => Ok(Self::PolicySink),
            [node_id] => Ok(Self::DirectSource {
                node_id: parse_nonzero(node_id, "source node id")?,
                process_id: source_process_argument()?,
            }),
            [mode, process_id, start_time, node_name] if mode == "--graph-route" => {
                if !valid_graph_output_name(node_name) {
                    return Err(SinkError::Fixture(
                        "graph observer received an unexpected output node name".into(),
                    ));
                }
                Ok(Self::GraphRoute {
                    route: GraphRouteExpectation {
                        process_id: parse_nonzero(process_id, "engine process id")?,
                        start_time: start_time.parse().map_err(|_| {
                            SinkError::Fixture("invalid engine process start time".into())
                        })?,
                        node_name: Rc::from(node_name.as_str()),
                    },
                })
            }
            _ => Err(SinkError::Fixture(
                "expected source node id, --policy-sink, or --graph-route identity".into(),
            )),
        }
    }

    const fn trace_config(&self) -> TraceConfig {
        match self {
            Self::DirectSource { .. } | Self::PolicySink => TraceConfig::source_mute(),
            Self::GraphRoute { .. } => TraceConfig::graph_continuity(),
        }
    }

    const fn sink_node_name(&self) -> &'static str {
        match self {
            Self::PolicySink => "io.mixrelay.fixture.policy-reroute-sink",
            Self::DirectSource { .. } | Self::GraphRoute { .. } => "io.mixrelay.fixture.mute-sink",
        }
    }

    const fn sink_application_id() -> &'static str {
        "org.mixrelay.fixture.mute-sink"
    }

    const fn sink_application_name() -> &'static str {
        "MixRelay Mute Boundary Sink"
    }
}

fn valid_graph_output_name(node_name: &str) -> bool {
    node_name == GRAPH_OUTPUT_NODE_NAME
        || node_name
            .strip_prefix("io.mixrelay.o_")
            .and_then(|name| name.strip_suffix("_native.device-output"))
            .is_some_and(|name| !name.is_empty())
}

struct ObserverControlContext<'a> {
    loop_: &'a MainLoopRc,
    core: &'a pipewire::core::CoreRc,
    stopped: &'a Cell<bool>,
}

fn parse_nonzero(value: &str, label: &'static str) -> Result<u32, SinkError> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| SinkError::Fixture(format!("invalid {label}")))
}

/// Runs a private input observer for a direct source check or an explicit graph route.
/// # Errors
/// Rejects out-of-lab execution, ambiguous commands, incomplete phases or native failures.
pub(super) fn run() -> Result<(), SinkError> {
    let executable = std::env::current_exe()?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let expected = std::env::var_os("MIXRELAY_LAB_APPLICATION_MUTE_SINK").map(PathBuf::from);
    if !is_owned_lab(&executable, expected.as_deref(), runtime.as_deref()) {
        return Err(SinkError::Fixture(
            "observer is outside its private lab".into(),
        ));
    }

    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let mode = ObserverMode::parse(&arguments)?;

    let loop_ = MainLoopRc::new(None)?;
    let stopped = Rc::new(Cell::new(false));
    let (_term, _interrupt) = register_stop_signals(&loop_, &stopped);
    let context = ContextRc::new(&loop_, None)?;
    let core = context.connect_rc(None)?;
    run_observer(&executable, &loop_, &core, &stopped, mode)
}

fn run_observer(
    executable: &Path,
    loop_: &MainLoopRc,
    core: &pipewire::core::CoreRc,
    stopped: &Cell<bool>,
    mode: ObserverMode,
) -> Result<(), SinkError> {
    let source_identity = match &mode {
        ObserverMode::DirectSource {
            node_id,
            process_id,
        } => Some(verify_source(core, loop_, *node_id, *process_id)?),
        ObserverMode::GraphRoute { .. } | ObserverMode::PolicySink => None,
    };
    let config = mode.trace_config();
    let process_id = std::process::id().to_string();
    let mut properties = properties! {
        "node.name" => mode.sink_node_name(),
        "media.class" => "Audio/Sink",
        "media.type" => "Audio", "media.category" => "Capture",
        "application.id" => ObserverMode::sink_application_id(),
        "application.process.id" => process_id.as_str(),
        "application.name" => ObserverMode::sink_application_name(),
        "node.virtual" => "true", "node.autoconnect" => "false",
        // DirectSource mute windows deliberately pause; GraphRoute windows keep
        // consuming the live stream so native SRC history survives arming.
        "node.dont-fallback" => "true", "node.pause-on-idle" => "true",
    };
    if matches!(&mode, ObserverMode::PolicySink) {
        properties.insert("priority.session", "1");
    }
    let stream = StreamBox::new(core, "MixRelay owned mute boundary sink", properties)?;
    let observer = CallbackSink::with_config(stream, config)?;
    // Keep setup failures on the same disconnect/trace-save path as control failures.
    let control_result = connect_and_control(
        &observer,
        &ObserverControlContext {
            loop_,
            core,
            stopped,
        },
        mode,
        source_identity,
        &process_id,
    );
    let trace_result = observer.finish();
    let saved = match trace_result {
        Ok(trace) => trace.save(executable, std::process::id()),
        Err(error) => Err(error),
    };
    merge_result(control_result, saved)
}

fn connect_and_control(
    observer: &CallbackSink<'_>,
    control: &ObserverControlContext<'_>,
    mode: ObserverMode,
    source_identity: Option<OwnedNodeProof>,
    process_id: &str,
) -> Result<(), SinkError> {
    let format = format_bytes()?;
    let pod = Pod::from_bytes(&format)
        .ok_or_else(|| SinkError::Fixture("bad audio format pod".into()))?;
    observer
        .stream()
        .connect(Direction::Input, None, OBSERVER_STREAM_FLAGS, &mut [pod])?;
    let sink_node = wait_for_node_id(observer.stream(), control.loop_, control.stopped)?;
    let sink_identity = verify_sink(
        control.core,
        control.loop_,
        sink_node,
        process_id,
        mode.sink_node_name(),
    )?;
    sink_identity.ensure_identity()?;
    let mut node_proofs = prepare_observer_proofs(control, mode, source_identity, sink_identity)?;
    let (_reader, commands) = command_receiver();
    control_loop(observer, control, &commands, &mut node_proofs)
}

fn prepare_observer_proofs(
    control: &ObserverControlContext<'_>,
    mode: ObserverMode,
    source_identity: Option<OwnedNodeProof>,
    sink_identity: OwnedNodeProof,
) -> Result<ObserverNodeProofs, SinkError> {
    match (mode, source_identity) {
        (ObserverMode::DirectSource { node_id, .. }, Some(source_identity)) => {
            let nodes = OwnedFixtureNodes {
                source: source_identity,
                sink: sink_identity,
            };
            let links =
                create_verified_links(control.core, control.loop_, node_id, nodes.sink.node_id)?;
            nodes.ensure_identity()?;
            announce_ready(
                nodes.sink.node_id,
                node_id,
                std::process::id(),
                &nodes,
                &links,
            )?;
            Ok(ObserverNodeProofs::Direct(nodes))
        }
        (ObserverMode::GraphRoute { route }, None) => {
            let sink_node = sink_identity.node_id;
            let graph = GraphRouteProof::new(control.core, sink_identity, sink_node, route)?;
            graph.sink.ensure_identity()?;
            announce_graph_ready(sink_node, std::process::id(), &graph.sink)?;
            Ok(ObserverNodeProofs::Graph(graph))
        }
        (ObserverMode::PolicySink, None) => {
            sink_identity.ensure_identity()?;
            write_control_line(&serde_json::json!({
                "event": "ready",
                "routeMode": "policy-sink",
                "nodeId": sink_identity.node_id,
                "nodeSerial": sink_identity.serial,
                "processId": std::process::id(),
                "applicationId": ObserverMode::sink_application_id(),
            }))?;
            Ok(ObserverNodeProofs::Policy(sink_identity))
        }
        _ => Err(SinkError::Fixture("observer mode identity mismatch".into())),
    }
}

fn announce_ready(
    sink_node: u32,
    source_node: u32,
    process_id: u32,
    nodes: &OwnedFixtureNodes,
    links: &[VerifiedLink],
) -> Result<(), SinkError> {
    write_control_line(&serde_json::json!({
        "event": "ready",
        "nodeId": sink_node,
        "nodeSerial": nodes.sink.serial.clone(),
        "sourceNodeId": source_node,
        "processId": process_id,
        "applicationId": "org.mixrelay.fixture.mute-sink",
        "linkIds": links.iter().map(|link| link.id).collect::<Vec<_>>(),
    }))
}

fn register_stop_signals<'a>(
    loop_: &'a MainLoopRc,
    stopped: &Rc<Cell<bool>>,
) -> (SignalSource<'a>, SignalSource<'a>) {
    let stop = Rc::clone(stopped);
    let term = loop_
        .loop_()
        .add_signal_local(Signal::TERM, move || stop.set(true));
    let stop = Rc::clone(stopped);
    let interrupt = loop_
        .loop_()
        .add_signal_local(Signal::INT, move || stop.set(true));
    (term, interrupt)
}

fn source_process_argument() -> Result<u32, SinkError> {
    std::env::var("MIXRELAY_LAB_APPLICATION_SOURCE_PID")
        .map_err(|_| SinkError::Fixture("missing owned source PID".into()))?
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid != 0)
        .ok_or_else(|| SinkError::Fixture("invalid owned source PID".into()))
}

fn wait_for_node_id(
    stream: &Stream,
    loop_: &MainLoopRc,
    stopped: &Cell<bool>,
) -> Result<u32, SinkError> {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        if stopped.get() {
            return Err(SinkError::Fixture(
                "observer stopped before PipeWire bound its node".into(),
            ));
        }
        match stream.state() {
            StreamState::Error(error) => {
                return Err(SinkError::Fixture(format!(
                    "observer failed before PipeWire bound its node: {error}"
                )));
            }
            StreamState::Unconnected => {
                return Err(SinkError::Fixture(
                    "observer became unconnected before PipeWire bound its node".into(),
                ));
            }
            StreamState::Connecting | StreamState::Paused | StreamState::Streaming => {}
        }
        let node_id = stream.node_id();
        if node_id != pipewire::spa::sys::SPA_ID_INVALID {
            return (node_id != 0)
                .then_some(node_id)
                .ok_or_else(|| SinkError::Fixture("PipeWire bound observer node id zero".into()));
        }
        if Instant::now() >= deadline {
            return Err(SinkError::Fixture(
                "timed out waiting for PipeWire observer node binding".into(),
            ));
        }
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
}

fn is_owned_lab(executable: &Path, expected: Option<&Path>, runtime: Option<&Path>) -> bool {
    executable.is_absolute()
        && expected == Some(executable)
        && executable.parent().is_some_and(|lab| {
            lab.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("app-lab-"))
                && runtime == Some(lab.join("runtime").as_path())
        })
}

fn format_bytes() -> Result<Vec<u8>, SinkError> {
    let mut audio = AudioInfoRaw::new();
    audio.set_format(if cfg!(target_endian = "little") {
        AudioFormat::F32LE
    } else {
        AudioFormat::F32BE
    });
    audio.set_rate(48_000);
    audio.set_channels(2);
    let mut position = [0; 64];
    position[0] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pipewire::spa::sys::SPA_AUDIO_CHANNEL_FR;
    audio.set_position(position);
    let format = Value::Object(Object {
        type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Format,
        id: ParamType::EnumFormat.as_raw(),
        properties: audio.into(),
    });
    PodSerializer::serialize(Cursor::new(Vec::new()), &format)
        .map(|(buffer, _)| buffer.into_inner())
        .map_err(|_| SinkError::Fixture("could not serialize sink format".into()))
}

#[derive(Clone)]
struct NodeIdentityExpectation {
    node_name: Rc<str>,
    media_class: &'static str,
    application_id: Option<&'static str>,
    process_id: String,
    node_group: Option<&'static str>,
    target_object: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NodeIdentitySnapshot {
    node_name: Option<String>,
    media_class: Option<String>,
    application_id: Option<String>,
    process_id: Option<String>,
    serial: Option<String>,
    node_group: Option<String>,
    target_object: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NodeIdentityObservation {
    Waiting(NodeIdentitySnapshot),
    Verified(NodeIdentitySnapshot),
    Rejected(String),
}

enum NodeBindingObservation {
    Waiting,
    Resolved(Result<Node, String>),
}

struct BoundNode {
    // Listener fields precede native handles so callbacks unregister before proxy release.
    registry_listener: RegistryListener,
    node: Node,
    removed: Rc<Cell<bool>>,
}

struct OwnedNodeProof {
    // Keep listeners before their native node; field drop order is declaration order.
    _info_listener: NodeListener,
    _registry_listener: RegistryListener,
    _node: Node,
    observation: Rc<RefCell<NodeIdentityObservation>>,
    expected: NodeIdentityExpectation,
    removed: Rc<Cell<bool>>,
    node_id: u32,
    serial: String,
}

struct OwnedFixtureNodes {
    source: OwnedNodeProof,
    sink: OwnedNodeProof,
}

struct GraphRouteProof {
    sink: OwnedNodeProof,
    monitor: GraphRouteMonitor,
    route: GraphRouteExpectation,
    source: Option<OwnedNodeProof>,
    ports: Option<OwnedPorts>,
    deadline: Instant,
    announced: bool,
}

enum ObserverNodeProofs {
    Direct(OwnedFixtureNodes),
    Graph(GraphRouteProof),
    Policy(OwnedNodeProof),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouteLinkState {
    Pending,
    Paused,
    Active,
    Failed,
    Removed,
}

struct ObservedRouteLink {
    endpoints: LinkEndpoints,
    state: RouteLinkState,
}

#[derive(Default)]
struct GraphRouteObservation {
    output_nodes: Vec<u32>,
    incoming_links: BTreeMap<u32, ObservedRouteLink>,
    error: Option<String>,
}

impl GraphRouteObservation {
    fn record_output_node(&mut self, node_id: u32) {
        if node_id == 0 || self.output_nodes.contains(&node_id) {
            return;
        }
        if self.output_nodes.is_empty() {
            self.output_nodes.push(node_id);
        } else {
            self.error = Some("duplicate engine output node name".into());
        }
    }
}

struct RetainedRouteLink {
    _listener: LinkListener,
    _link: Link,
}

struct GraphRouteMonitor {
    // Listener is dropped before retained link proxies.
    _registry_listener: RegistryListener,
    _links: Rc<RefCell<Vec<RetainedRouteLink>>>,
    observation: Rc<RefCell<GraphRouteObservation>>,
    sink_node_id: u32,
}

struct GraphRouteCallbackContext {
    output_node_name: Rc<str>,
    sink_node_id: u32,
    registry: pipewire::registry::RegistryRc,
    observation: Rc<RefCell<GraphRouteObservation>>,
    retained_links: Rc<RefCell<Vec<RetainedRouteLink>>>,
}

impl GraphRouteProof {
    fn new(
        core: &pipewire::core::CoreRc,
        sink: OwnedNodeProof,
        sink_node_id: u32,
        route: GraphRouteExpectation,
    ) -> Result<Self, SinkError> {
        verify_process_start(&route)?;
        Ok(Self {
            sink,
            monitor: GraphRouteMonitor::new(core, sink_node_id, Rc::clone(&route.node_name))?,
            route,
            source: None,
            ports: None,
            deadline: Instant::now() + ROUTE_TIMEOUT,
            announced: false,
        })
    }

    fn ensure_ready(
        &mut self,
        core: &pipewire::core::CoreRc,
        loop_: &MainLoopRc,
        stage: PhaseStage,
    ) -> Result<bool, SinkError> {
        self.sink.ensure_identity()?;
        verify_process_start(&self.route)?;
        if self.source.is_none() {
            let Some(node_id) = self.monitor.output_node_id()? else {
                return self.route_timeout_or_wait();
            };
            self.source = Some(verify_graph_output(
                core,
                loop_,
                node_id,
                &self.route,
                &self.sink.serial,
            )?);
            self.ports = Some(wait_for_owned_ports(
                core,
                loop_,
                node_id,
                self.sink.node_id,
            )?);
        }
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| SinkError::Fixture("verified graph output is missing".into()))?;
        source.ensure_identity()?;
        let ports = self
            .ports
            .as_ref()
            .ok_or_else(|| SinkError::Fixture("verified graph ports are missing".into()))?;
        let ready = self
            .monitor
            .route_is_ready(source.node_id, self.sink.node_id, ports, stage)?;
        if !ready {
            if self.announced && !matches!(stage, PhaseStage::Resuming { .. }) {
                return Err(SinkError::Fixture(
                    "engine-to-observer route changed during the continuity window".into(),
                ));
            }
            if matches!(stage, PhaseStage::Resuming { .. }) {
                return Ok(false);
            }
            return self.route_timeout_or_wait();
        }
        if !self.announced {
            write_control_line(&serde_json::json!({
                "event": "routeReady",
                "sourceNodeId": source.node_id,
                "sourceNodeSerial": source.serial.clone(),
                "sinkNodeId": self.sink.node_id,
                "sinkNodeSerial": self.sink.serial.clone(),
                "linkIds": self.monitor.incoming_link_ids(),
                "processId": self.route.process_id,
                "processStartTime": self.route.start_time,
            }))?;
            self.announced = true;
        }
        Ok(true)
    }

    fn route_timeout_or_wait(&self) -> Result<bool, SinkError> {
        if Instant::now() >= self.deadline {
            return Err(SinkError::Fixture(format!(
                "graph route was not proven within bounded setup: {}",
                self.monitor.diagnostic()
            )));
        }
        Ok(false)
    }
}

impl ObserverNodeProofs {
    fn ensure_ready(
        &mut self,
        core: &pipewire::core::CoreRc,
        loop_: &MainLoopRc,
        stage: PhaseStage,
    ) -> Result<bool, SinkError> {
        match self {
            Self::Direct(nodes) => {
                nodes.ensure_identity()?;
                Ok(true)
            }
            Self::Graph(route) => route.ensure_ready(core, loop_, stage),
            Self::Policy(sink) => {
                sink.ensure_identity()?;
                Ok(true)
            }
        }
    }
}

fn verify_process_start(route: &GraphRouteExpectation) -> Result<(), SinkError> {
    let actual = process_start_time(route.process_id)?;
    if actual != route.start_time {
        return Err(SinkError::Fixture(format!(
            "graph owner PID {} start time changed (expected {}, found {actual})",
            route.process_id, route.start_time
        )));
    }
    Ok(())
}

fn process_start_time(process_id: u32) -> Result<u64, SinkError> {
    let stat = std::fs::read_to_string(format!("/proc/{process_id}/stat"))?;
    let fields = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect::<Vec<_>>())
        .ok_or_else(|| SinkError::Fixture("malformed process stat identity".into()))?;
    fields
        .get(19)
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| SinkError::Fixture("missing process start-time identity".into()))
}

impl GraphRouteMonitor {
    fn new(
        core: &pipewire::core::CoreRc,
        sink_node_id: u32,
        output_node_name: Rc<str>,
    ) -> Result<Self, SinkError> {
        let registry = core.get_registry_rc()?;
        let observation = Rc::new(RefCell::new(GraphRouteObservation::default()));
        let links = Rc::new(RefCell::new(Vec::with_capacity(
            MAX_OBSERVED_INCOMING_LINKS,
        )));
        let callback_context = GraphRouteCallbackContext {
            output_node_name,
            sink_node_id,
            registry: registry.clone(),
            observation: Rc::clone(&observation),
            retained_links: Rc::clone(&links),
        };
        let listener = registry
            .add_listener_local()
            .global(move |object| observe_route_global(object, &callback_context))
            .global_remove({
                let removed = Rc::clone(&observation);
                move |node_id| observe_route_removal(&removed, node_id)
            })
            .register();
        Ok(Self {
            _registry_listener: listener,
            _links: links,
            observation,
            sink_node_id,
        })
    }

    fn output_node_id(&self) -> Result<Option<u32>, SinkError> {
        let observation = self.observation.borrow();
        if let Some(error) = &observation.error {
            return Err(SinkError::Fixture(error.clone()));
        }
        match observation.output_nodes.as_slice() {
            [] => Ok(None),
            [node_id] => Ok(Some(*node_id)),
            _ => Err(SinkError::Fixture(
                "multiple engine output nodes matched the exact graph identity".into(),
            )),
        }
    }

    fn route_is_ready(
        &self,
        source_node_id: u32,
        sink_node_id: u32,
        ports: &OwnedPorts,
        stage: PhaseStage,
    ) -> Result<bool, SinkError> {
        if sink_node_id != self.sink_node_id {
            return Err(SinkError::Fixture("observer sink identity changed".into()));
        }
        let observation = self.observation.borrow();
        if let Some(error) = &observation.error {
            return Err(SinkError::Fixture(error.clone()));
        }
        let expected = graph_link_endpoints(source_node_id, sink_node_id, ports)?;
        route_links_ready(&observation.incoming_links, expected, stage)
    }

    fn incoming_link_ids(&self) -> Vec<u32> {
        self.observation
            .borrow()
            .incoming_links
            .keys()
            .copied()
            .collect()
    }

    fn diagnostic(&self) -> String {
        let observation = self.observation.borrow();
        format!(
            "source_candidates={}, incoming_links={}, observer_sink={}",
            observation.output_nodes.len(),
            observation.incoming_links.len(),
            self.sink_node_id
        )
    }
}

fn route_link_is_active(link: &ObservedRouteLink, stage: PhaseStage) -> Result<bool, SinkError> {
    match link.state {
        RouteLinkState::Pending => Ok(false),
        RouteLinkState::Paused => Ok(matches!(stage, PhaseStage::Pausing { .. })),
        RouteLinkState::Active => Ok(true),
        RouteLinkState::Failed => Err(SinkError::Fixture(
            "engine output link entered a native error state".into(),
        )),
        RouteLinkState::Removed => Err(SinkError::Fixture(
            "engine output link was removed before the continuity window".into(),
        )),
    }
}

fn route_links_ready(
    incoming: &BTreeMap<u32, ObservedRouteLink>,
    expected: [LinkEndpoints; 2],
    stage: PhaseStage,
) -> Result<bool, SinkError> {
    if incoming.len() > 2 {
        return Err(SinkError::Fixture(
            "owned graph sink has extra incoming links".into(),
        ));
    }
    if incoming.len() != 2 {
        return Ok(false);
    }
    let mut seen = [false; 2];
    let mut active = [false; 2];
    for link in incoming.values() {
        let index = if link.endpoints == expected[0] {
            0
        } else if link.endpoints == expected[1] {
            1
        } else {
            return Err(SinkError::Fixture(
                "owned graph sink has a mixed or bypass input link".into(),
            ));
        };
        if seen[index] {
            return Err(SinkError::Fixture(
                "duplicate engine output stereo link".into(),
            ));
        }
        seen[index] = true;
        active[index] = route_link_is_active(link, stage)?;
    }
    Ok(seen.into_iter().all(|value| value) && active.into_iter().all(|value| value))
}

fn graph_link_endpoints(
    source_node_id: u32,
    sink_node_id: u32,
    ports: &OwnedPorts,
) -> Result<[LinkEndpoints; 2], SinkError> {
    let endpoint = |channel| -> Result<LinkEndpoints, SinkError> {
        Ok(LinkEndpoints {
            output_node: source_node_id,
            output_port: *ports.source.get(channel).ok_or_else(|| {
                SinkError::Fixture(format!("engine output has no {channel} port"))
            })?,
            input_node: sink_node_id,
            input_port: *ports.sink.get(channel).ok_or_else(|| {
                SinkError::Fixture(format!("private observer has no {channel} port"))
            })?,
        })
    };
    Ok([endpoint("FL")?, endpoint("FR")?])
}

fn observe_route_removal(observation: &RefCell<GraphRouteObservation>, id: u32) {
    if let Some(link) = observation.borrow_mut().incoming_links.get_mut(&id) {
        link.state = RouteLinkState::Removed;
    }
}

fn observe_route_global(
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    context: &GraphRouteCallbackContext,
) {
    match object.type_ {
        ObjectType::Node => observe_output_node(object, context),
        ObjectType::Link => {
            observe_incoming_link(object, context);
        }
        _ => {}
    }
}

fn observe_output_node(
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    context: &GraphRouteCallbackContext,
) {
    let matches = object
        .props
        .as_ref()
        .and_then(|properties| properties.get("node.name"))
        == Some(context.output_node_name.as_ref());
    if !matches {
        return;
    }
    context
        .observation
        .borrow_mut()
        .record_output_node(object.id);
}

fn observe_incoming_link(
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    context: &GraphRouteCallbackContext,
) {
    let Some(properties) = object.props.as_ref() else {
        context.observation.borrow_mut().error =
            Some("PipeWire link identity has no properties".into());
        return;
    };
    let Ok(endpoints) = parse_link_endpoints(properties) else {
        context.observation.borrow_mut().error = Some("PipeWire link identity is malformed".into());
        return;
    };
    if endpoints.input_node != context.sink_node_id {
        return;
    }
    retain_incoming_link(object, endpoints, context);
}

fn parse_link_endpoints(
    properties: &pipewire::spa::utils::dict::DictRef,
) -> Result<LinkEndpoints, ()> {
    let number = |key| {
        properties
            .get(key)
            .and_then(|value| value.parse::<u32>().ok())
    };
    let output_node = number("link.output.node")
        .filter(|value| *value != 0)
        .ok_or(())?;
    let output_port = number("link.output.port")
        .filter(|value| *value != 0)
        .ok_or(())?;
    let input_node = number("link.input.node")
        .filter(|value| *value != 0)
        .ok_or(())?;
    let input_port = number("link.input.port")
        .filter(|value| *value != 0)
        .ok_or(())?;
    Ok(LinkEndpoints {
        output_node,
        output_port,
        input_node,
        input_port,
    })
}

fn retain_incoming_link(
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    endpoints: LinkEndpoints,
    context: &GraphRouteCallbackContext,
) {
    let id = object.id;
    let mut state = context.observation.borrow_mut();
    if id == 0 || state.incoming_links.contains_key(&id) {
        state.error = Some("duplicate or zero graph route link id".into());
        return;
    }
    if state.incoming_links.len() >= MAX_OBSERVED_INCOMING_LINKS {
        state.error = Some("graph route link inventory exceeded its fixed bound".into());
        return;
    }
    state.incoming_links.insert(
        id,
        ObservedRouteLink {
            endpoints,
            state: RouteLinkState::Pending,
        },
    );
    drop(state);
    bind_incoming_link(object, context, id, endpoints);
}

fn bind_incoming_link(
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    context: &GraphRouteCallbackContext,
    id: u32,
    endpoints: LinkEndpoints,
) {
    let Ok(link) = context.registry.bind::<Link, _>(object) else {
        context.observation.borrow_mut().error =
            Some("could not bind exact graph route link".into());
        return;
    };
    let callback_observation = Rc::clone(&context.observation);
    let listener = link
        .add_listener_local()
        .info({
            move |info| {
                observe_route_link_info(&callback_observation, id, endpoints, info);
            }
        })
        .register();
    context.retained_links.borrow_mut().push(RetainedRouteLink {
        _listener: listener,
        _link: link,
    });
}

fn observe_route_link_info(
    observation: &RefCell<GraphRouteObservation>,
    expected_id: u32,
    expected_endpoints: LinkEndpoints,
    info: &pipewire::link::LinkInfoRef,
) {
    let mut observation = observation.borrow_mut();
    let Some(link) = observation.incoming_links.get_mut(&expected_id) else {
        observation.error = Some("bound graph link has no registry identity".into());
        return;
    };
    let endpoints = LinkEndpoints {
        output_node: info.output_node_id(),
        output_port: info.output_port_id(),
        input_node: info.input_node_id(),
        input_port: info.input_port_id(),
    };
    if info.id() != expected_id || endpoints != expected_endpoints || endpoints != link.endpoints {
        observation.error = Some("bound graph link identity changed".into());
        return;
    }
    link.state = match info.state() {
        LinkState::Active => RouteLinkState::Active,
        LinkState::Paused => RouteLinkState::Paused,
        LinkState::Error(_) | LinkState::Unlinked => RouteLinkState::Failed,
        LinkState::Init | LinkState::Negotiating | LinkState::Allocating => RouteLinkState::Pending,
    };
}

fn announce_graph_ready(
    sink_node: u32,
    process_id: u32,
    sink: &OwnedNodeProof,
) -> Result<(), SinkError> {
    write_control_line(&serde_json::json!({
        "event": "ready",
        "nodeId": sink_node,
        "nodeSerial": sink.serial,
        "processId": process_id,
        "applicationId": "org.mixrelay.fixture.mute-sink",
        "routeMode": "graph",
    }))
}

struct NodeInfoWait {
    listener: NodeListener,
    observation: Rc<RefCell<NodeIdentityObservation>>,
    state: NodeIdentityObservation,
}

impl NodeIdentitySnapshot {
    fn from_info(info: &NodeInfoRef) -> Self {
        let properties = info.props();
        let property = |name| properties.and_then(|props| props.get(name).map(str::to_owned));
        Self {
            node_name: property("node.name"),
            media_class: property("media.class"),
            application_id: property("application.id"),
            process_id: property("application.process.id"),
            serial: property("object.serial"),
            node_group: property("node.group"),
            target_object: properties.and_then(|props| {
                props
                    .get("target.object")
                    .or_else(|| props.get("node.target"))
                    .map(str::to_owned)
            }),
        }
    }

    fn conflicts_with(&self, expected: &NodeIdentityExpectation) -> bool {
        self.node_name
            .as_deref()
            .is_some_and(|value| value != expected.node_name.as_ref())
            || self
                .media_class
                .as_deref()
                .is_some_and(|value| value != expected.media_class)
            || expected.application_id.is_some_and(|expected| {
                self.application_id
                    .as_deref()
                    .is_some_and(|value| value != expected)
            })
            || self
                .process_id
                .as_deref()
                .is_some_and(|value| value != expected.process_id)
            || self.serial.as_deref() == Some("")
            || expected.node_group.is_some_and(|expected| {
                self.node_group
                    .as_deref()
                    .is_some_and(|value| value != expected)
            })
            || expected.target_object.as_deref().is_some_and(|expected| {
                self.target_object
                    .as_deref()
                    .is_some_and(|value| value != expected)
            })
    }

    fn matches(&self, expected: &NodeIdentityExpectation) -> bool {
        self.node_name.as_deref() == Some(expected.node_name.as_ref())
            && self.media_class.as_deref() == Some(expected.media_class)
            && expected
                .application_id
                .is_none_or(|expected| self.application_id.as_deref() == Some(expected))
            && self.process_id.as_deref() == Some(expected.process_id.as_str())
            && self
                .serial
                .as_deref()
                .is_some_and(|serial| !serial.is_empty())
            && expected
                .node_group
                .is_none_or(|expected| self.node_group.as_deref() == Some(expected))
            && expected
                .target_object
                .as_deref()
                .is_none_or(|expected| self.target_object.as_deref() == Some(expected))
    }

    fn diagnostic(&self, node_id: u32, expected: &NodeIdentityExpectation) -> String {
        format!(
            "node {node_id} identity did not verify (expected pid {}; node.name={}, media.class={}, application.id={}, application.process.id={}, object.serial={}, node.group={}, target.object={})",
            expected.process_id,
            diagnostic_property(self.node_name.as_deref()),
            diagnostic_property(self.media_class.as_deref()),
            diagnostic_property(self.application_id.as_deref()),
            diagnostic_property(self.process_id.as_deref()),
            diagnostic_property(self.serial.as_deref()),
            diagnostic_property(self.node_group.as_deref()),
            diagnostic_property(self.target_object.as_deref())
        )
    }
}

fn diagnostic_property(value: Option<&str>) -> String {
    const MAX_BYTES: usize = 96;
    let Some(value) = value else {
        return "<missing>".into();
    };
    let mut chars = value.chars();
    let mut escaped = String::with_capacity(MAX_BYTES);
    let mut truncated = false;
    for character in chars.by_ref() {
        let piece = match character {
            '\\' => Some("\\\\"),
            '"' => Some("\\\""),
            character if character.is_ascii_graphic() || character == ' ' => None,
            _ => Some("?"),
        };
        let piece_len = match piece {
            Some(piece) => piece.len(),
            None => 1,
        };
        if escaped.len() + piece_len > MAX_BYTES {
            truncated = true;
            break;
        }
        if let Some(piece) = piece {
            escaped.push_str(piece);
        } else {
            escaped.push(character);
        }
    }
    truncated |= chars.next().is_some();
    let suffix = if truncated { "…" } else { "" };
    format!("\"{escaped}{suffix}\"")
}

fn verify_source(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
    process_id: u32,
) -> Result<OwnedNodeProof, SinkError> {
    verify_node(
        core,
        loop_,
        node_id,
        NodeIdentityExpectation {
            node_name: Rc::from("io.mixrelay.lab-application"),
            media_class: "Stream/Output/Audio",
            application_id: Some("org.mixrelay.fixture.application"),
            process_id: process_id.to_string(),
            node_group: None,
            target_object: None,
        },
    )
}

fn verify_sink(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
    process_id: &str,
    node_name: &'static str,
) -> Result<OwnedNodeProof, SinkError> {
    verify_node(
        core,
        loop_,
        node_id,
        NodeIdentityExpectation {
            node_name: Rc::from(node_name),
            media_class: "Audio/Sink",
            application_id: Some("org.mixrelay.fixture.mute-sink"),
            process_id: process_id.to_owned(),
            node_group: None,
            target_object: None,
        },
    )
}

fn verify_graph_output(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
    route: &GraphRouteExpectation,
    sink_serial: &str,
) -> Result<OwnedNodeProof, SinkError> {
    verify_node(
        core,
        loop_,
        node_id,
        NodeIdentityExpectation {
            node_name: Rc::clone(&route.node_name),
            media_class: "Stream/Output/Audio",
            application_id: None,
            process_id: route.process_id.to_string(),
            node_group: Some("mixrelay.graph"),
            target_object: Some(sink_serial.to_owned()),
        },
    )
}

fn verify_node(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
    expected: NodeIdentityExpectation,
) -> Result<OwnedNodeProof, SinkError> {
    let bound = bind_exact_node(core, loop_, node_id)?;
    verify_node_info(bound, loop_, node_id, expected)
}

fn bind_exact_node(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
) -> Result<BoundNode, SinkError> {
    if node_id == 0 {
        return Err(SinkError::Fixture("native node id is zero".into()));
    }
    let registry = core.get_registry_rc()?;
    let binding = Rc::new(RefCell::new(NodeBindingObservation::Waiting));
    let callback_binding = Rc::clone(&binding);
    let bind_registry = registry.clone();
    let removed = Rc::new(Cell::new(false));
    let callback_bind_removed = Rc::clone(&removed);
    let callback_remove_flag = Rc::clone(&removed);
    let listener = registry
        .add_listener_local()
        .global(move |object| {
            if object.id != node_id || object.type_ != ObjectType::Node {
                return;
            }
            if !matches!(&*callback_binding.borrow(), NodeBindingObservation::Waiting) {
                return;
            }
            let result = bind_registry
                .bind::<Node, _>(object)
                .map_err(|error| format!("bind exact PipeWire node {node_id}: {error}"));
            *callback_binding.borrow_mut() = NodeBindingObservation::Resolved(result);
            if callback_bind_removed.get() {
                *callback_binding.borrow_mut() = NodeBindingObservation::Resolved(Err(format!(
                    "exact PipeWire node {node_id} was removed while binding"
                )));
            }
        })
        .global_remove(move |removed_id| {
            if removed_id == node_id {
                callback_remove_flag.set(true);
            }
        })
        .register();
    let node = wait_for_bound_node(loop_, &binding, &removed, node_id)?;
    Ok(BoundNode {
        registry_listener: listener,
        node,
        removed,
    })
}

fn wait_for_bound_node(
    loop_: &MainLoopRc,
    binding: &RefCell<NodeBindingObservation>,
    removed: &Cell<bool>,
    node_id: u32,
) -> Result<Node, SinkError> {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while matches!(&*binding.borrow(), NodeBindingObservation::Waiting)
        && !removed.get()
        && Instant::now() < deadline
    {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    let state = {
        let mut state = binding.borrow_mut();
        std::mem::replace(&mut *state, NodeBindingObservation::Waiting)
    };
    match state {
        NodeBindingObservation::Waiting if removed.get() => Err(SinkError::Fixture(format!(
            "exact PipeWire node {node_id} was removed before binding"
        ))),
        NodeBindingObservation::Waiting => Err(SinkError::Fixture(format!(
            "timed out binding exact PipeWire node {node_id}"
        ))),
        NodeBindingObservation::Resolved(Ok(_)) if removed.get() => Err(SinkError::Fixture(
            format!("exact PipeWire node {node_id} was removed while binding"),
        )),
        NodeBindingObservation::Resolved(Ok(node)) => Ok(node),
        NodeBindingObservation::Resolved(Err(error)) => Err(SinkError::Fixture(error)),
    }
}

fn verify_node_info(
    bound: BoundNode,
    loop_: &MainLoopRc,
    node_id: u32,
    expected: NodeIdentityExpectation,
) -> Result<OwnedNodeProof, SinkError> {
    let observation = Rc::new(RefCell::new(NodeIdentityObservation::Waiting(
        NodeIdentitySnapshot::default(),
    )));
    let callback_observation = Rc::clone(&observation);
    let callback_expected = expected.clone();
    let listener = bound
        .node
        .add_listener_local()
        .info(move |info| {
            let previous = callback_observation.borrow().clone();
            let next = observe_node_info(info, node_id, &callback_expected, &previous);
            if !matches!(&previous, NodeIdentityObservation::Rejected(_)) {
                *callback_observation.borrow_mut() = next;
            }
        })
        .register();
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while matches!(&*observation.borrow(), NodeIdentityObservation::Waiting(_))
        && !bound.removed.get()
        && Instant::now() < deadline
    {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    let state = observation.borrow().clone();
    owned_node_proof(
        bound,
        NodeInfoWait {
            listener,
            observation,
            state,
        },
        expected,
        node_id,
    )
}

fn observe_node_info(
    info: &NodeInfoRef,
    node_id: u32,
    expected: &NodeIdentityExpectation,
    previous: &NodeIdentityObservation,
) -> NodeIdentityObservation {
    let previous_snapshot = match previous {
        NodeIdentityObservation::Waiting(snapshot)
        | NodeIdentityObservation::Verified(snapshot) => snapshot,
        NodeIdentityObservation::Rejected(error) => {
            return NodeIdentityObservation::Rejected(error.clone());
        }
    };
    if info.id() != node_id {
        return NodeIdentityObservation::Rejected(format!(
            "{}; native NodeInfo id was {}",
            previous_snapshot.diagnostic(node_id, expected),
            info.id()
        ));
    }
    if info.change_mask().contains(NodeChangeMask::STATE)
        && let NodeState::Error(error) = info.state()
    {
        return NodeIdentityObservation::Rejected(format!(
            "{}; native node error: {}",
            previous_snapshot.diagnostic(node_id, expected),
            diagnostic_property(Some(error))
        ));
    }
    let identity = info
        .change_mask()
        .contains(NodeChangeMask::PROPS)
        .then(|| NodeIdentitySnapshot::from_info(info));
    observe_node_properties(identity, expected, node_id, previous)
}

fn observe_node_properties(
    actual: Option<NodeIdentitySnapshot>,
    expected: &NodeIdentityExpectation,
    node_id: u32,
    previous: &NodeIdentityObservation,
) -> NodeIdentityObservation {
    let Some(actual) = actual else {
        return previous.clone();
    };
    if actual.conflicts_with(expected) {
        return NodeIdentityObservation::Rejected(actual.diagnostic(node_id, expected));
    }
    if let NodeIdentityObservation::Verified(previous_identity) = previous
        && actual.serial != previous_identity.serial
    {
        return NodeIdentityObservation::Rejected(actual.diagnostic(node_id, expected));
    }
    if actual.matches(expected) {
        return NodeIdentityObservation::Verified(actual);
    }
    if matches!(previous, NodeIdentityObservation::Verified(_)) {
        return NodeIdentityObservation::Rejected(actual.diagnostic(node_id, expected));
    }
    NodeIdentityObservation::Waiting(actual)
}

fn owned_node_proof(
    bound: BoundNode,
    waited: NodeInfoWait,
    expected: NodeIdentityExpectation,
    node_id: u32,
) -> Result<OwnedNodeProof, SinkError> {
    match waited.state {
        NodeIdentityObservation::Verified(identity) if !bound.removed.get() => {
            let serial = identity.serial.clone().unwrap_or_default();
            Ok(OwnedNodeProof {
                _info_listener: waited.listener,
                _registry_listener: bound.registry_listener,
                _node: bound.node,
                observation: waited.observation,
                expected,
                removed: bound.removed,
                node_id,
                serial,
            })
        }
        NodeIdentityObservation::Rejected(error) => Err(SinkError::Fixture(error)),
        NodeIdentityObservation::Verified(identity)
        | NodeIdentityObservation::Waiting(identity) => {
            let removed = if bound.removed.get() {
                "; native node was removed during identity verification"
            } else {
                ""
            };
            Err(SinkError::Fixture(format!(
                "{}{removed}",
                identity.diagnostic(node_id, &expected)
            )))
        }
    }
}

impl OwnedNodeProof {
    fn ensure_identity(&self) -> Result<(), SinkError> {
        if self.removed.get() {
            return Err(SinkError::Fixture(format!(
                "{}; native node was removed",
                self.current_diagnostic()
            )));
        }
        match &*self.observation.borrow() {
            NodeIdentityObservation::Verified(identity) if identity.matches(&self.expected) => {
                Ok(())
            }
            NodeIdentityObservation::Rejected(error) => Err(SinkError::Fixture(error.clone())),
            NodeIdentityObservation::Waiting(identity)
            | NodeIdentityObservation::Verified(identity) => Err(SinkError::Fixture(
                identity.diagnostic(self.node_id, &self.expected),
            )),
        }
    }

    fn current_diagnostic(&self) -> String {
        match &*self.observation.borrow() {
            NodeIdentityObservation::Waiting(identity)
            | NodeIdentityObservation::Verified(identity) => {
                identity.diagnostic(self.node_id, &self.expected)
            }
            NodeIdentityObservation::Rejected(error) => error.clone(),
        }
    }
}

impl OwnedFixtureNodes {
    fn ensure_identity(&self) -> Result<(), SinkError> {
        self.source.ensure_identity()?;
        self.sink.ensure_identity()
    }
}

#[derive(Clone)]
struct OwnedPorts {
    source: BTreeMap<&'static str, u32>,
    sink: BTreeMap<&'static str, u32>,
    sink_monitor: BTreeMap<&'static str, u32>,
    error: Option<String>,
}

impl OwnedPorts {
    fn new() -> Self {
        Self {
            source: BTreeMap::new(),
            sink: BTreeMap::new(),
            sink_monitor: BTreeMap::new(),
            error: None,
        }
    }

    fn complete(&self) -> bool {
        self.source.len() == 2 && self.sink.len() == 2
    }
}

fn wait_for_owned_ports(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    source_node: u32,
    sink_node: u32,
) -> Result<OwnedPorts, SinkError> {
    let registry = core.get_registry_rc()?;
    let found = Rc::new(RefCell::new(OwnedPorts::new()));
    let callback_found = Rc::clone(&found);
    let listener: RegistryListener = registry
        .add_listener_local()
        .global(move |object| {
            if object.type_ != ObjectType::Port {
                return;
            }
            let mut ports = callback_found.borrow_mut();
            if ports.error.is_some() {
                return;
            }
            if let Err(error) = add_owned_port(&mut ports, object, source_node, sink_node) {
                ports.error = Some(error);
            }
        })
        .register();
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let complete = {
            let ports = found.borrow();
            ports.complete() || ports.error.is_some()
        };
        if complete || Instant::now() >= deadline {
            break;
        }
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    drop(listener);
    let ports = found.borrow().clone();
    if let Some(error) = ports.error.as_ref() {
        return Err(SinkError::Fixture(error.clone()));
    }
    if !ports.complete() {
        return Err(SinkError::Fixture(
            "timed out finding exact stereo ports on owned nodes".into(),
        ));
    }
    Ok(ports)
}

fn add_owned_port(
    ports: &mut OwnedPorts,
    object: &pipewire::registry::GlobalObject<&pipewire::spa::utils::dict::DictRef>,
    source_node: u32,
    sink_node: u32,
) -> Result<(), String> {
    let Some(properties) = object.props.as_ref() else {
        return Ok(());
    };
    add_owned_port_properties(
        ports,
        OwnedPortProperties {
            object_id: object.id,
            node_id: properties.get("node.id"),
            direction: properties.get("port.direction"),
            monitor: properties.get("port.monitor"),
            channel: properties.get("audio.channel"),
        },
        OwnedPortNodes {
            source: source_node,
            sink: sink_node,
        },
    )
}

#[derive(Clone, Copy)]
struct OwnedPortProperties<'a> {
    object_id: u32,
    node_id: Option<&'a str>,
    direction: Option<&'a str>,
    monitor: Option<&'a str>,
    channel: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct OwnedPortNodes {
    source: u32,
    sink: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnedPortKind {
    SourceOutput,
    SinkInput,
    SinkMonitorOutput,
}

fn add_owned_port_properties(
    ports: &mut OwnedPorts,
    properties: OwnedPortProperties<'_>,
    nodes: OwnedPortNodes,
) -> Result<(), String> {
    let Some(node_id) = properties
        .node_id
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return Ok(());
    };
    let Some(kind) = classify_owned_port(node_id, properties.direction, properties.monitor, nodes)?
    else {
        return Ok(());
    };
    let channel = match properties.channel {
        Some("FL") => "FL",
        Some("FR") => "FR",
        _ => {
            return Err(format!(
                "owned port {id} has no stereo channel",
                id = properties.object_id
            ));
        }
    };
    if properties.object_id == 0 || ports.contains_port_id(properties.object_id) {
        return Err(format!(
            "owned port id {} is ambiguous",
            properties.object_id
        ));
    }
    let target = match kind {
        OwnedPortKind::SourceOutput => &mut ports.source,
        OwnedPortKind::SinkInput => &mut ports.sink,
        OwnedPortKind::SinkMonitorOutput => &mut ports.sink_monitor,
    };
    if target.contains_key(channel) {
        return Err(format!("owned port channel {channel} is ambiguous"));
    }
    target.insert(channel, properties.object_id);
    Ok(())
}

impl OwnedPorts {
    fn contains_port_id(&self, object_id: u32) -> bool {
        self.source.values().any(|id| *id == object_id)
            || self.sink.values().any(|id| *id == object_id)
            || self.sink_monitor.values().any(|id| *id == object_id)
    }
}

fn classify_owned_port(
    node_id: u32,
    direction: Option<&str>,
    monitor: Option<&str>,
    nodes: OwnedPortNodes,
) -> Result<Option<OwnedPortKind>, String> {
    if node_id != nodes.source && node_id != nodes.sink {
        return Ok(None);
    }
    let direction =
        direction.ok_or_else(|| format!("owned port on node {node_id} has no direction"))?;
    let monitor = match monitor {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => {
            return Err(format!(
                "owned port on node {node_id} has malformed monitor flag"
            ));
        }
    };
    let kind = if node_id == nodes.source && direction == "out" && !monitor {
        OwnedPortKind::SourceOutput
    } else if node_id == nodes.sink && direction == "in" && !monitor {
        OwnedPortKind::SinkInput
    } else if node_id == nodes.sink && direction == "out" && monitor {
        OwnedPortKind::SinkMonitorOutput
    } else {
        return Err(format!(
            "owned port on node {node_id} has an unexpected direction/monitor combination"
        ));
    };
    Ok(Some(kind))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkEndpoints {
    output_node: u32,
    output_port: u32,
    input_node: u32,
    input_port: u32,
}

struct VerifiedLink {
    id: u32,
    _listener: LinkListener,
    _link: Link,
}

fn create_verified_links(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    source_node: u32,
    sink_node: u32,
) -> Result<Vec<VerifiedLink>, SinkError> {
    let ports = wait_for_owned_ports(core, loop_, source_node, sink_node)?;
    let mut links = Vec::with_capacity(2);
    for channel in ["FL", "FR"] {
        let endpoints = LinkEndpoints {
            output_node: source_node,
            output_port: *ports.source.get(channel).expect("verified source channel"),
            input_node: sink_node,
            input_port: *ports.sink.get(channel).expect("verified sink channel"),
        };
        links.push(create_active_link(core, loop_, endpoints)?);
    }
    Ok(links)
}

fn create_active_link(
    core: &pipewire::core::CoreRc,
    loop_: &MainLoopRc,
    endpoints: LinkEndpoints,
) -> Result<VerifiedLink, SinkError> {
    let output_node = endpoints.output_node.to_string();
    let output_port = endpoints.output_port.to_string();
    let input_node = endpoints.input_node.to_string();
    let input_port = endpoints.input_port.to_string();
    let link = core.create_object::<Link>(
        "link-factory",
        &properties! {
            "link.output.node" => output_node.as_str(),
            "link.output.port" => output_port.as_str(),
            "link.input.node" => input_node.as_str(),
            "link.input.port" => input_port.as_str(),
        },
    )?;
    let (listener, id) = wait_for_active_link(&link, loop_, endpoints)?;
    Ok(VerifiedLink {
        id,
        _listener: listener,
        _link: link,
    })
}

fn wait_for_active_link(
    link: &Link,
    loop_: &MainLoopRc,
    endpoints: LinkEndpoints,
) -> Result<(LinkListener, u32), SinkError> {
    let state = Rc::new(Cell::new(0_u8));
    let callback_state = Rc::clone(&state);
    let id = Rc::new(Cell::new(0_u32));
    let callback_id = Rc::clone(&id);
    let listener = link
        .add_listener_local()
        .info(move |info| {
            if info.output_node_id() != endpoints.output_node
                || info.output_port_id() != endpoints.output_port
                || info.input_node_id() != endpoints.input_node
                || info.input_port_id() != endpoints.input_port
            {
                callback_state.set(2);
                return;
            }
            match info.state() {
                LinkState::Active => {
                    callback_id.set(info.id());
                    callback_state.set(1);
                }
                LinkState::Error(_) => callback_state.set(2),
                _ => {}
            }
        })
        .register();
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while state.get() == 0 && Instant::now() < deadline {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    if state.get() != 1 || id.get() == 0 {
        return Err(SinkError::Fixture(
            "owned source-to-sink link did not become active with exact ports".into(),
        ));
    }
    Ok((listener, id.get()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhaseStage {
    Ready,
    WaitingForPcm { phase: usize, deadline: Instant },
    Pausing { phase: usize, deadline: Instant },
    Resuming { phase: usize, deadline: Instant },
    Capturing { phase: usize, deadline: Instant },
}

impl PhaseStage {
    fn begin_pause(self, phase: usize, now: Instant) -> Result<Self, SinkError> {
        if self != Self::Ready {
            return Err(SinkError::Fixture(
                "phase command overlaps active capture".into(),
            ));
        }
        Ok(Self::Pausing {
            phase,
            deadline: now + SETUP_TIMEOUT,
        })
    }

    fn begin_resume(self, now: Instant) -> Result<Self, SinkError> {
        let Self::Pausing {
            phase,
            deadline: pause_deadline,
        } = self
        else {
            return Err(SinkError::Fixture(
                "capture resumed without a paused reset boundary".into(),
            ));
        };
        if now >= pause_deadline {
            return Err(SinkError::Fixture(
                "timed out waiting for native sink to pause before phase".into(),
            ));
        }
        Ok(Self::Resuming {
            phase,
            deadline: now + SETUP_TIMEOUT,
        })
    }

    fn begin_capture(self, now: Instant) -> Result<Self, SinkError> {
        let (Self::Resuming { phase, deadline } | Self::WaitingForPcm { phase, deadline }) = self
        else {
            return Err(SinkError::Fixture(
                "capture armed before native observer resumed".into(),
            ));
        };
        if now >= deadline {
            return Err(SinkError::Fixture(
                "timed out waiting for native sink route to resume before phase".into(),
            ));
        }
        Ok(Self::Capturing {
            phase,
            deadline: now + COMMAND_TIMEOUT,
        })
    }

    fn complete(self, phase: usize) -> Result<Self, SinkError> {
        if !matches!(self, Self::Capturing { phase: active, .. } if active == phase) {
            return Err(SinkError::Fixture(
                "native PCM completed outside the armed phase".into(),
            ));
        }
        Ok(Self::Ready)
    }

    fn is_ready(self) -> bool {
        self == Self::Ready
    }

    fn can_arm(self, pcm_buffers: u64) -> bool {
        matches!(self, Self::Resuming { .. })
            || (matches!(self, Self::WaitingForPcm { .. }) && pcm_buffers != 0)
    }

    fn ensure_before_deadline(self, now: Instant) -> Result<(), SinkError> {
        let (deadline, error) = match self {
            Self::Ready => return Ok(()),
            Self::Pausing { deadline, .. } => (
                deadline,
                "timed out waiting for native sink to pause before phase",
            ),
            Self::Resuming { deadline, .. } => (
                deadline,
                "timed out waiting for native sink route to resume before phase",
            ),
            Self::WaitingForPcm { deadline, .. } => (
                deadline,
                "timed out waiting for running graph observer PCM before phase",
            ),
            Self::Capturing { deadline, .. } => {
                (deadline, "phase did not collect its bounded PCM window")
            }
        };
        if now >= deadline {
            return Err(SinkError::Fixture(error.into()));
        }
        Ok(())
    }
}

struct ControlProgress {
    next_phase: usize,
    phase_count: usize,
    stage: PhaseStage,
    announced_complete: usize,
}

impl ControlProgress {
    fn new(phase_count: usize) -> Self {
        Self {
            next_phase: 1,
            phase_count,
            stage: PhaseStage::Ready,
            announced_complete: 0,
        }
    }

    fn observe_completion(&mut self, completed: usize) -> Result<bool, SinkError> {
        if completed > self.phase_count {
            return Err(SinkError::Fixture(
                "native PCM completed beyond the configured phase count".into(),
            ));
        }
        if completed <= self.announced_complete {
            return Ok(false);
        }
        self.stage = self.stage.complete(completed)?;
        self.announced_complete = completed;
        self.next_phase = completed + 1;
        Ok(true)
    }
}

fn control_loop(
    observer: &CallbackSink<'_>,
    control: &ObserverControlContext<'_>,
    commands: &Receiver<Result<String, String>>,
    node_proofs: &mut ObserverNodeProofs,
) -> Result<(), SinkError> {
    let state = observer.phase_state();
    let config = observer.config();
    let mut progress = ControlProgress::new(config.phase_count);
    while !control.stopped.get() {
        control
            .loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
        progress.stage.ensure_before_deadline(Instant::now())?;
        if !node_proofs.ensure_ready(control.core, control.loop_, progress.stage)? {
            continue;
        }
        if state.invalid_buffers.load(Ordering::Acquire) != 0 {
            return Err(SinkError::Fixture("invalid native PCM buffer".into()));
        }
        arm_requested_phase(observer, state, &mut progress.stage)?;
        if progress.observe_completion(state.completed.load(Ordering::Acquire))? {
            write_control_line(&serde_json::json!({
                "event": "phaseComplete",
                "phase": progress.announced_complete,
                "frames": config.phase_frames,
            }))?;
        }
        match commands.try_recv() {
            Ok(Ok(command)) if command == "quit" => {
                control.stopped.set(true);
            }
            Ok(Ok(command)) => {
                begin_requested_phase(
                    observer,
                    state,
                    &mut progress,
                    &command,
                    matches!(node_proofs, ObserverNodeProofs::Graph(_)),
                )?;
            }
            Ok(Err(error)) => return Err(SinkError::Fixture(error)),
            Err(TryRecvError::Disconnected) => control.stopped.set(true),
            Err(TryRecvError::Empty) => {}
        }
    }
    Ok(())
}

fn arm_requested_phase(
    observer: &CallbackSink<'_>,
    callback: &PhaseState,
    stage: &mut PhaseStage,
) -> Result<(), SinkError> {
    let (PhaseStage::Pausing { phase, deadline }
    | PhaseStage::Resuming { phase, deadline }
    | PhaseStage::WaitingForPcm { phase, deadline }) = *stage
    else {
        return Ok(());
    };
    if Instant::now() >= deadline {
        return Err(SinkError::Fixture(format!(
            "timed out waiting for native sink to arm phase {phase}: {:?}",
            observer.stream().state()
        )));
    }
    match observer.stream().state() {
        StreamState::Paused if matches!(stage, PhaseStage::Pausing { .. }) => {
            // Input streams do not provide a DRAINED boundary. Flush this observer's
            // input queue while paused; this is not a graph-wide upstream drain.
            observer.stream().flush(false)?;
            *stage = stage.begin_resume(Instant::now())?;
            observer.stream().set_active(true)?;
            Ok(())
        }
        StreamState::Streaming if stage.can_arm(callback.pcm_buffers.load(Ordering::Acquire)) => {
            // The control loop has re-proven active exact links before capture starts.
            *stage = stage.begin_capture(Instant::now())?;
            callback.active.store(phase, Ordering::Release);
            write_control_line(&serde_json::json!({
                "event": "armed",
                "phase": phase,
            }))
        }
        StreamState::Error(error) => Err(SinkError::Fixture(format!(
            "native sink failed before phase {phase} could arm: {error}"
        ))),
        StreamState::Unconnected => Err(SinkError::Fixture(format!(
            "native sink disconnected before phase {phase} could arm"
        ))),
        StreamState::Connecting | StreamState::Paused | StreamState::Streaming => Ok(()),
    }
}

fn begin_requested_phase(
    observer: &CallbackSink<'_>,
    state: &PhaseState,
    progress: &mut ControlProgress,
    command: &str,
    running_graph: bool,
) -> Result<(), SinkError> {
    if !progress.stage.is_ready() {
        return Err(SinkError::Fixture(
            "phase command overlaps active capture".into(),
        ));
    }
    let phase = parse_phase(command, observer.config().phase_count)?;
    if phase != progress.next_phase || phase > observer.config().phase_count {
        return Err(SinkError::Fixture(format!(
            "phase must be the next value {}",
            progress.next_phase
        )));
    }
    state.active.store(0, Ordering::Release);
    if running_graph {
        // This is a window in an already running graph, not a mute transition.
        // Pausing/flushing would reset the observer's native SRC history. Keep
        // dequeuing outside the phase and arm only after a real PCM callback.
        progress.stage = PhaseStage::WaitingForPcm {
            phase,
            deadline: Instant::now() + SETUP_TIMEOUT,
        };
        return Ok(());
    }
    observer.stream().set_active(false)?;
    progress.stage = progress.stage.begin_pause(phase, Instant::now())?;
    Ok(())
}

fn parse_phase(command: &str, phase_count: usize) -> Result<usize, SinkError> {
    command
        .strip_prefix("phase ")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|phase| (1..=phase_count).contains(phase))
        .ok_or_else(|| SinkError::Fixture("invalid phase command".into()))
}

fn merge_result(
    primary: Result<(), SinkError>,
    save: Result<(), SinkError>,
) -> Result<(), SinkError> {
    match (primary, save) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(save)) => Err(save),
        (Err(primary), Err(save)) => Err(SinkError::Fixture(format!(
            "control failed: {primary}; teardown/evidence save failed: {save}"
        ))),
    }
}

#[cfg(test)]
mod phase_tests {
    use super::*;

    #[test]
    fn observer_dispatch_is_synchronous_without_autoconnect_or_async_buffering() {
        // PipeWire 1.4.2: mapped buffers (bit 2), RT callback (bit 4), no other flags.
        assert_eq!(OBSERVER_STREAM_FLAGS.bits(), (1 << 2) | (1 << 4));
    }

    #[test]
    fn capture_requires_a_paused_reset_boundary_and_rejects_overlap() {
        let now = Instant::now();
        let pausing = PhaseStage::Ready.begin_pause(1, now).unwrap();
        assert!(!pausing.is_ready());
        assert!(pausing.begin_pause(2, now).is_err());
        assert!(PhaseStage::Ready.begin_capture(now).is_err());

        assert!(pausing.begin_capture(now).is_err());
        assert!(pausing.begin_resume(now + SETUP_TIMEOUT).is_err());
        let capture_start = now + SETUP_TIMEOUT - Duration::from_nanos(1);
        let resuming = pausing.begin_resume(capture_start).unwrap();
        assert!(resuming.complete(1).is_err());
        assert!(
            resuming
                .begin_capture(capture_start + SETUP_TIMEOUT)
                .is_err()
        );
        let capturing = resuming.begin_capture(capture_start).unwrap();
        assert!(!capturing.is_ready());
        assert!(capturing.begin_capture(now).is_err());
        assert_eq!(capturing.complete(1).unwrap(), PhaseStage::Ready);
        assert!(capturing.complete(2).is_err());
    }

    #[test]
    fn running_graph_arms_after_callback_without_pause_or_flush() {
        let now = Instant::now();
        let waiting = PhaseStage::WaitingForPcm {
            phase: 1,
            deadline: now + SETUP_TIMEOUT,
        };
        assert!(!waiting.can_arm(0));
        assert!(waiting.can_arm(1));
        assert!(waiting.begin_pause(1, now).is_err());
        assert!(waiting.begin_resume(now).is_err());
        assert!(waiting.begin_capture(now + SETUP_TIMEOUT).is_err());
        let capturing = waiting.begin_capture(now).unwrap();
        assert!(!capturing.can_arm(1));
        assert_eq!(capturing.complete(1).unwrap(), PhaseStage::Ready);
        let paused = PhaseStage::Ready.begin_pause(1, now).unwrap();
        assert!(!paused.can_arm(1));
        assert!(paused.begin_resume(now).unwrap().can_arm(0));
    }

    #[test]
    fn pause_and_capture_deadlines_are_bounded_and_stage_specific() {
        let now = Instant::now();
        let pausing = PhaseStage::Ready.begin_pause(1, now).unwrap();
        assert!(
            pausing
                .ensure_before_deadline(now + SETUP_TIMEOUT - Duration::from_nanos(1))
                .is_ok()
        );
        let pause_error = pausing
            .ensure_before_deadline(now + SETUP_TIMEOUT)
            .unwrap_err();
        assert!(
            pause_error
                .to_string()
                .contains("timed out waiting for native sink to pause")
        );

        let capture_start = now + SETUP_TIMEOUT - Duration::from_nanos(1);
        let resuming = pausing.begin_resume(capture_start).unwrap();
        assert!(
            resuming
                .ensure_before_deadline(capture_start + SETUP_TIMEOUT)
                .unwrap_err()
                .to_string()
                .contains("route to resume")
        );
        let capturing = resuming.begin_capture(capture_start).unwrap();
        assert!(
            capturing
                .ensure_before_deadline(capture_start + COMMAND_TIMEOUT - Duration::from_nanos(1))
                .is_ok()
        );
        let capture_error = capturing
            .ensure_before_deadline(capture_start + COMMAND_TIMEOUT)
            .unwrap_err();
        assert!(capture_error.to_string().contains("bounded PCM window"));
        assert!(
            PhaseStage::Ready
                .ensure_before_deadline(capture_start + COMMAND_TIMEOUT)
                .is_ok()
        );
    }

    #[test]
    fn control_and_trace_save_failures_are_both_preserved() {
        let error = merge_result(
            Err(SinkError::Fixture("phase failed".into())),
            Err(SinkError::Fixture("trace save failed".into())),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("phase failed"));
        assert!(error.contains("trace save failed"));
    }

    #[test]
    fn completion_advances_only_the_current_armed_phase_once() {
        let now = Instant::now();
        let mut progress = ControlProgress::new(3);
        assert!(!progress.observe_completion(0).unwrap());
        assert!(progress.observe_completion(1).is_err());
        assert_eq!(progress.next_phase, 1);
        progress.stage = progress.stage.begin_pause(1, now).unwrap();
        assert!(progress.observe_completion(1).is_err());
        progress.stage = progress.stage.begin_resume(now).unwrap();
        assert!(progress.observe_completion(1).is_err());
        progress.stage = progress.stage.begin_capture(now).unwrap();
        assert!(progress.observe_completion(2).is_err());
        assert!(progress.observe_completion(1).unwrap());
        assert_eq!(progress.next_phase, 2);
        assert!(progress.stage.is_ready());
        assert!(!progress.observe_completion(1).unwrap());
        assert!(progress.observe_completion(3).is_err());
        assert_eq!(progress.next_phase, 2);
    }
}

fn command_receiver() -> (thread::JoinHandle<()>, Receiver<Result<String, String>>) {
    let (sender, receiver) = sync_channel(1);
    let worker = thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            match read_control_line(&mut input) {
                Ok(Some(line)) => {
                    if sender.send(Ok(line)).is_err() {
                        return;
                    }
                }
                Ok(None) => return,
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            }
        }
    });
    (worker, receiver)
}

fn read_control_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    const MAX_CONTROL_BYTES: usize = 64;
    let mut bytes = Vec::with_capacity(MAX_CONTROL_BYTES);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "unterminated control line",
                ))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        let content = newline.map_or(available, |index| &available[..index]);
        if bytes.len() + content.len() > MAX_CONTROL_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "control line exceeds 64 bytes",
            ));
        }
        bytes.extend_from_slice(content);
        reader.consume(consumed);
        if newline.is_some() {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
    }
}

fn write_control_line(value: &serde_json::Value) -> Result<(), SinkError> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn fixture_expectation() -> NodeIdentityExpectation {
        NodeIdentityExpectation {
            node_name: Rc::from("io.mixrelay.lab-application"),
            media_class: "Stream/Output/Audio",
            application_id: Some("org.mixrelay.fixture.application"),
            process_id: "1234".into(),
            node_group: None,
            target_object: None,
        }
    }

    #[test]
    fn node_properties_wait_for_complete_identity_then_reject_replacement() {
        let expected = fixture_expectation();
        let partial = NodeIdentitySnapshot {
            node_name: Some(expected.node_name.to_string()),
            media_class: Some(expected.media_class.into()),
            serial: Some("17".into()),
            ..NodeIdentitySnapshot::default()
        };
        let waiting = observe_node_properties(
            Some(partial.clone()),
            &expected,
            42,
            &NodeIdentityObservation::Waiting(NodeIdentitySnapshot::default()),
        );
        assert_eq!(waiting, NodeIdentityObservation::Waiting(partial));

        let complete = NodeIdentitySnapshot {
            node_name: Some(expected.node_name.to_string()),
            media_class: Some(expected.media_class.into()),
            application_id: expected.application_id.map(str::to_owned),
            process_id: Some(expected.process_id.clone()),
            serial: Some("123".into()),
            ..NodeIdentitySnapshot::default()
        };
        let verified = observe_node_properties(Some(complete.clone()), &expected, 42, &waiting);
        assert_eq!(
            verified,
            NodeIdentityObservation::Verified(complete.clone())
        );
        assert_eq!(
            observe_node_properties(None, &expected, 42, &verified),
            verified,
            "state-only node info must preserve a verified properties snapshot"
        );

        let mut replacement = complete.clone();
        replacement.process_id = Some("5678".into());
        assert!(matches!(
            observe_node_properties(Some(replacement), &expected, 42, &verified),
            NodeIdentityObservation::Rejected(_)
        ));

        let mut changed_serial = complete;
        changed_serial.serial = Some("456".into());
        assert!(matches!(
            observe_node_properties(Some(changed_serial), &expected, 42, &verified),
            NodeIdentityObservation::Rejected(_)
        ));
    }

    #[test]
    fn identity_diagnostics_bound_only_the_owned_property_fields() {
        let expected = fixture_expectation();
        let snapshot = NodeIdentitySnapshot {
            node_name: Some("n".repeat(256)),
            ..NodeIdentitySnapshot::default()
        };
        let diagnostic = snapshot.diagnostic(42, &expected);
        assert!(diagnostic.chars().count() < 600);
        assert!(diagnostic.contains("application.id=<missing>"));

        let long_error = diagnostic_property(Some(&"e".repeat(256)));
        assert!(long_error.chars().count() <= 100);
        assert!(long_error.ends_with("…\""));
    }

    #[test]
    fn graph_output_identity_requires_exact_group_and_private_sink_target() {
        let expected = NodeIdentityExpectation {
            node_name: Rc::from(GRAPH_OUTPUT_NODE_NAME),
            media_class: "Stream/Output/Audio",
            application_id: None,
            process_id: "1234".into(),
            node_group: Some("mixrelay.graph"),
            target_object: Some("private-sink-serial".into()),
        };
        let verified = NodeIdentitySnapshot {
            node_name: Some(GRAPH_OUTPUT_NODE_NAME.into()),
            media_class: Some("Stream/Output/Audio".into()),
            process_id: Some("1234".into()),
            serial: Some("output-serial".into()),
            node_group: Some("mixrelay.graph".into()),
            target_object: Some("private-sink-serial".into()),
            ..NodeIdentitySnapshot::default()
        };
        assert!(verified.matches(&expected));
        for (node_group, target_object) in [
            (Some("foreign.group"), Some("private-sink-serial")),
            (Some("mixrelay.graph"), Some("default-sink")),
            (None, Some("private-sink-serial")),
            (Some("mixrelay.graph"), None),
        ] {
            let changed = NodeIdentitySnapshot {
                node_group: node_group.map(str::to_owned),
                target_object: target_object.map(str::to_owned),
                ..verified.clone()
            };
            assert!(!changed.matches(&expected));
        }
        let wrong_process = NodeIdentitySnapshot {
            process_id: Some("4321".into()),
            ..verified
        };
        assert!(!wrong_process.matches(&expected));
        assert!(wrong_process.conflicts_with(&expected));
    }
}

#[cfg(test)]
mod owned_port_tests {
    use super::*;

    const NODES: OwnedPortNodes = OwnedPortNodes {
        source: 39,
        sink: 51,
    };

    fn port<'a>(
        object_id: u32,
        node_id: &'a str,
        direction: Option<&'a str>,
        monitor: Option<&'a str>,
        channel: Option<&'a str>,
    ) -> OwnedPortProperties<'a> {
        OwnedPortProperties {
            object_id,
            node_id: Some(node_id),
            direction,
            monitor,
            channel,
        }
    }

    #[test]
    fn accepts_owned_stereo_ports_and_ignores_only_sink_monitor_outputs_for_links() {
        let mut ports = OwnedPorts::new();
        for properties in [
            port(40, "39", Some("out"), None, Some("FL")),
            port(41, "39", Some("out"), None, Some("FR")),
            port(52, "51", Some("in"), None, Some("FL")),
            port(54, "51", Some("in"), Some("false"), Some("FR")),
            port(53, "51", Some("out"), Some("true"), Some("FL")),
            port(55, "51", Some("out"), Some("true"), Some("FR")),
        ] {
            add_owned_port_properties(&mut ports, properties, NODES).unwrap();
        }

        assert!(ports.complete());
        assert_eq!(ports.source, BTreeMap::from([("FL", 40), ("FR", 41)]));
        assert_eq!(ports.sink, BTreeMap::from([("FL", 52), ("FR", 54)]));
        assert_eq!(ports.sink_monitor, BTreeMap::from([("FL", 53), ("FR", 55)]));
    }

    #[test]
    fn ignores_ports_from_unrelated_nodes_before_validating_their_properties() {
        let mut ports = OwnedPorts::new();
        add_owned_port_properties(
            &mut ports,
            port(77, "700", None, Some("not-a-bool"), None),
            NODES,
        )
        .unwrap();
        assert!(ports.source.is_empty());
        assert!(ports.sink.is_empty());
        assert!(ports.sink_monitor.is_empty());
    }

    #[test]
    fn rejects_missing_malformed_or_unexpected_owned_port_direction_and_monitor() {
        let invalid = [
            port(60, "51", None, None, Some("FL")),
            port(67, "51", Some("sideways"), Some("false"), Some("FL")),
            port(61, "51", Some("out"), None, Some("FL")),
            port(62, "51", Some("out"), Some("yes"), Some("FL")),
            port(63, "51", Some("out"), Some("false"), Some("FL")),
            port(64, "51", Some("in"), Some("true"), Some("FL")),
            port(65, "39", Some("in"), None, Some("FL")),
            port(66, "39", Some("out"), Some("true"), Some("FL")),
        ];
        for properties in invalid {
            assert!(add_owned_port_properties(&mut OwnedPorts::new(), properties, NODES).is_err());
        }
    }

    #[test]
    fn rejects_bad_ids_channels_and_duplicate_owned_channels_without_replacing_evidence() {
        let mut ports = OwnedPorts::new();
        assert!(
            add_owned_port_properties(
                &mut ports,
                port(0, "51", Some("in"), None, Some("FL")),
                NODES,
            )
            .is_err()
        );
        assert!(
            add_owned_port_properties(
                &mut ports,
                port(70, "51", Some("in"), None, Some("FC")),
                NODES,
            )
            .is_err()
        );
        add_owned_port_properties(
            &mut ports,
            port(71, "51", Some("out"), Some("true"), Some("FL")),
            NODES,
        )
        .unwrap();
        assert!(
            add_owned_port_properties(
                &mut ports,
                port(72, "51", Some("out"), Some("true"), Some("FL")),
                NODES,
            )
            .is_err()
        );
        assert_eq!(ports.sink_monitor.get("FL"), Some(&71));
        assert!(
            add_owned_port_properties(
                &mut ports,
                port(71, "39", Some("out"), None, Some("FL")),
                NODES,
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod graph_route_tests {
    use super::*;

    fn endpoints(channel_offset: u32) -> [LinkEndpoints; 2] {
        [
            LinkEndpoints {
                output_node: 10,
                output_port: 20 + channel_offset,
                input_node: 30,
                input_port: 40 + channel_offset,
            },
            LinkEndpoints {
                output_node: 10,
                output_port: 21 + channel_offset,
                input_node: 30,
                input_port: 41 + channel_offset,
            },
        ]
    }

    fn link(endpoints: LinkEndpoints, state: RouteLinkState) -> ObservedRouteLink {
        ObservedRouteLink { endpoints, state }
    }

    #[test]
    fn route_requires_exact_active_stereo_links_without_foreign_inputs() {
        let expected = endpoints(0);
        assert!(!route_links_ready(&BTreeMap::new(), expected, PhaseStage::Ready).unwrap());
        let active = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[1], RouteLinkState::Active)),
        ]);
        assert!(route_links_ready(&active, expected, PhaseStage::Ready).unwrap());

        let pending = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[1], RouteLinkState::Pending)),
        ]);
        assert!(!route_links_ready(&pending, expected, PhaseStage::Ready).unwrap());

        let mixed = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (
                51,
                link(
                    LinkEndpoints {
                        output_node: 99,
                        ..expected[1]
                    },
                    RouteLinkState::Active,
                ),
            ),
        ]);
        assert!(route_links_ready(&mixed, expected, PhaseStage::Ready).is_err());
    }

    #[test]
    fn route_rejects_duplicate_extra_and_removed_links() {
        let expected = endpoints(0);
        let duplicate = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[0], RouteLinkState::Active)),
        ]);
        assert!(route_links_ready(&duplicate, expected, PhaseStage::Ready).is_err());

        let extra = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[1], RouteLinkState::Active)),
            (52, link(expected[1], RouteLinkState::Active)),
        ]);
        assert!(route_links_ready(&extra, expected, PhaseStage::Ready).is_err());

        let removed = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[1], RouteLinkState::Removed)),
        ]);
        assert!(route_links_ready(&removed, expected, PhaseStage::Ready).is_err());
    }

    #[test]
    fn exact_paused_links_are_accepted_only_during_the_requested_reset() {
        let expected = endpoints(0);
        let paused = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Paused)),
            (51, link(expected[1], RouteLinkState::Paused)),
        ]);
        let now = Instant::now();
        let pausing = PhaseStage::Ready.begin_pause(1, now).unwrap();
        let resuming = pausing.begin_resume(now).unwrap();
        let capturing = resuming.begin_capture(now).unwrap();
        assert!(route_links_ready(&paused, expected, pausing).unwrap());
        for stage in [PhaseStage::Ready, resuming, capturing] {
            assert!(!route_links_ready(&paused, expected, stage).unwrap());
        }

        let pending = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Pending)),
            (51, link(expected[1], RouteLinkState::Paused)),
        ]);
        assert!(!route_links_ready(&pending, expected, pausing).unwrap());
        let foreign = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Paused)),
            (51, link(endpoints(1)[1], RouteLinkState::Paused)),
        ]);
        assert!(route_links_ready(&foreign, expected, pausing).is_err());
        let active = BTreeMap::from([
            (50, link(expected[0], RouteLinkState::Active)),
            (51, link(expected[1], RouteLinkState::Active)),
        ]);
        assert!(route_links_ready(&active, expected, resuming).unwrap());
    }

    #[test]
    fn graph_route_mode_requires_exact_node_identity_and_nonzero_process_id() {
        let valid = vec![
            "--graph-route".into(),
            "1234".into(),
            "9876".into(),
            GRAPH_OUTPUT_NODE_NAME.into(),
        ];
        assert!(matches!(
            ObserverMode::parse(&valid),
            Ok(ObserverMode::GraphRoute { .. })
        ));
        let composed_name =
            "io.mixrelay.o_39_device_29_io.mixrelay.fixture.mute-sink_native.device-output";
        let composed = vec![
            "--graph-route".into(),
            "1234".into(),
            "9876".into(),
            composed_name.into(),
        ];
        let Ok(ObserverMode::GraphRoute { route }) = ObserverMode::parse(&composed) else {
            panic!("canonical composed output name must be accepted");
        };
        assert_eq!(route.node_name.as_ref(), composed_name);

        for invalid in [
            vec![
                "--graph-route".into(),
                "0".into(),
                "1".into(),
                GRAPH_OUTPUT_NODE_NAME.into(),
            ],
            vec![
                "--graph-route".into(),
                "1234".into(),
                "bad".into(),
                GRAPH_OUTPUT_NODE_NAME.into(),
            ],
            vec![
                "--graph-route".into(),
                "1234".into(),
                "1".into(),
                "io.foreign.node".into(),
            ],
            vec![
                "--graph-route".into(),
                "1234".into(),
                "1".into(),
                "io.mixrelay.o__native.device-output".into(),
            ],
        ] {
            assert!(ObserverMode::parse(&invalid).is_err());
        }
    }

    #[test]
    fn duplicate_graph_output_candidates_are_rejected() {
        let mut observed = GraphRouteObservation::default();
        observed.record_output_node(70);
        observed.record_output_node(70);
        assert_eq!(observed.output_nodes.as_slice(), &[70]);
        assert!(observed.error.is_none());
        observed.record_output_node(71);
        assert_eq!(
            observed.error.as_deref(),
            Some("duplicate engine output node name")
        );
    }

    #[test]
    fn policy_sink_mode_never_selects_the_client_linked_direct_source() {
        let mode = ObserverMode::PolicySink;
        assert_eq!(
            mode.sink_node_name(),
            "io.mixrelay.fixture.policy-reroute-sink"
        );
        assert!(ObserverMode::parse(&["--policy-sink".into(), "1".into()]).is_err());
    }

    #[test]
    fn policy_sink_single_argument_does_not_parse_as_a_direct_source() {
        assert!(matches!(
            ObserverMode::parse(&["--policy-sink".into()]),
            Ok(ObserverMode::PolicySink)
        ));
    }
}
