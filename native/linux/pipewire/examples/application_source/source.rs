use super::{
    callback_source::{CallbackSource, Dispatch},
    tone::Rate,
};
use pipewire::{
    context::ContextRc,
    core::{CoreRc, Listener as CoreListener},
    loop_::{Signal, Timeout},
    main_loop::MainLoopRc,
    node::{Node, NodeChangeMask, NodeInfoRef, NodeListener, NodeState},
    properties::properties,
    proxy::ProxyT,
    registry::Listener as RegistryListener,
    spa::{
        param::{
            ParamType,
            audio::{AudioFormat, AudioInfoRaw},
        },
        pod::{Object, Pod, Property, Value, serialize::PodSerializer},
        utils::{
            Direction,
            result::{SpaResult, SpaSuccess},
        },
    },
    stream::{StreamBox, StreamFlags, StreamState},
    types::ObjectType,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::{BufRead, Cursor, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{Receiver, TryRecvError, sync_channel},
    thread,
    time::{Duration, Instant},
};

/// A fixture failure, reported only after leaving the callback/event loop.
#[derive(Debug, thiserror::Error)]
pub(super) enum SourceError {
    /// Native context, stream or listener setup failed.
    #[error("native application source: {0}")]
    Native(#[from] pipewire::Error),
    /// Owned trace file could not be created or flushed.
    #[error("application source trace: {0}")]
    Io(#[from] std::io::Error),
    /// Trace serialization failed.
    #[error("application source JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The requested source rate is outside the two fixture cases.
    #[error("application fixture supports only 44100 and 48000 Hz")]
    Rate,
    /// Native format could not be represented as a SPA pod.
    #[error("application source format serialization failed")]
    Format,
    /// A lab source cannot be launched against an unspecified user session.
    #[error("application source requires its owned private lab runtime and executable")]
    Scope,
    /// The diagnostic axis accepts only the two native dispatch modes.
    #[error("application source dispatch must be main or data")]
    Dispatch,
    /// A disconnected callback failed to return its trace to the control owner.
    #[error("application source trace handback after drain: {0}")]
    TraceReturn(std::sync::mpsc::TryRecvError),
    /// The owned-source mute boundary could not be identified, controlled or read back.
    #[error("application source mute fixture: {0}")]
    Mute(String),
    /// Both the control operation and restoring the source's original mute value failed.
    #[error("source control failed: {control}; mute restoration failed: {restore}")]
    ControlAndRestore {
        control: Box<SourceError>,
        restore: Box<SourceError>,
    },
}

fn source_rate(value: Option<&str>) -> Result<Rate, SourceError> {
    match value {
        Some("44100") => Ok(Rate::Hz44100),
        None | Some("48000") => Ok(Rate::Hz48000),
        Some(_) => Err(SourceError::Rate),
    }
}

fn format_bytes(rate: Rate) -> Result<Vec<u8>, SourceError> {
    let mut audio = AudioInfoRaw::new();
    audio.set_format(if cfg!(target_endian = "little") {
        AudioFormat::F32LE
    } else {
        AudioFormat::F32BE
    });
    audio.set_rate(u32::from(rate.hz()));
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
        .map_err(|_| SourceError::Format)
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

/// Runs one lab-owned source and saves trace after native teardown.
/// # Errors
/// Returns invalid rate, native creation, format or trace-file errors.
pub(super) fn run() -> Result<(), SourceError> {
    let configuration = source_configuration()?;
    run_source(&configuration)
}

struct SourceConfiguration {
    mute_boundary: bool,
    policy_route: Option<String>,
    executable: PathBuf,
    rate: Rate,
    dispatch: Dispatch,
}

fn source_configuration() -> Result<SourceConfiguration, SourceError> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let (mute_boundary, policy_route) = match arguments.as_slice() {
        [] => (false, None),
        [flag] if flag == "--mute-boundary" => (true, None),
        [flag, target] if flag == "--policy-route" && policy_route_target(target) => {
            (false, Some(target.clone()))
        }
        _ => return Err(SourceError::Mute("unsupported fixture arguments".into())),
    };
    let executable = std::env::current_exe()?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let expected =
        std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE").map(std::path::PathBuf::from);
    if !is_owned_lab(&executable, expected.as_deref(), runtime.as_deref()) {
        return Err(SourceError::Scope);
    }
    let rate = source_rate(std::env::var("MIXRELAY_LAB_SOURCE_RATE").ok().as_deref())?;
    let dispatch = Dispatch::parse(
        std::env::var("MIXRELAY_LAB_SOURCE_DISPATCH")
            .ok()
            .as_deref(),
    )?;
    Ok(SourceConfiguration {
        mute_boundary,
        policy_route,
        executable,
        rate,
        dispatch,
    })
}

fn policy_route_target(target: &str) -> bool {
    target
        .strip_prefix("io.mixrelay.endpoint.")
        .and_then(|hash| hash.strip_suffix(".render"))
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn run_source(configuration: &SourceConfiguration) -> Result<(), SourceError> {
    let loop_ = MainLoopRc::new(None)?;
    // Native signal sources block signals on this thread. Register before the
    // context starts any data thread, so every child inherits that mask; an
    // unmasked data thread would take SIGTERM's default process termination.
    let stopped = Rc::new(Cell::new(false));
    let stop = stopped.clone();
    let term = loop_
        .loop_()
        .add_signal_local(Signal::TERM, move || stop.set(true));
    let stop = stopped.clone();
    let interrupt = loop_
        .loop_()
        .add_signal_local(Signal::INT, move || stop.set(true));
    // Both dispatch cases use installed client.conf and the same native geometry;
    // selecting the data callback does not select a different OS priority policy.
    let context = ContextRc::new(&loop_, None)?;
    let core = context.connect_rc(None)?;
    let source = create_source(
        &core,
        configuration.rate,
        configuration.dispatch,
        configuration.policy_route.as_deref(),
    )?;
    let control = if configuration.mute_boundary {
        let source_node = wait_for_node_id(&source, &loop_, &stopped)?;
        // The product controls another application's node through a separate client connection.
        // Keep the fixture source's export connection independent of its control proxy too.
        // SourceMute retains this CoreRc, which in turn retains the context and owning loop.
        let controller = context.connect_rc(Some(properties! {
            "application.name" => "MixRelay source mute fixture controller",
        }))?;
        Some(SourceMute::new(&controller, &loop_, source_node)?)
    } else {
        None
    };
    let control_result = run_source_control(&loop_, &stopped, &source, control.as_ref());
    let restore_result = control.as_ref().map_or(Ok(()), |control| {
        control.set_and_confirm(control.original, &loop_, "restore-original")
    });
    let control_restore_result = combine_control_and_restore(control_result, restore_result);
    let trace = source.finish()?;
    drop(core);
    drop(context);
    drop((term, interrupt));
    // The listener/native child are gone before reading or serializing state.
    let path = configuration
        .executable
        .with_file_name(format!("source-{}.json", std::process::id()));
    trace.save(
        &path,
        configuration.rate.hz(),
        configuration.dispatch.label(),
    )?;
    control_restore_result?;
    if let Some(control) = control.as_ref() {
        write_control_line(&serde_json::json!({
            "event": "restored",
            "mute": control.original,
        }))?;
    }
    Ok(())
}

fn combine_control_and_restore(
    control_result: Result<Option<SourceError>, SourceError>,
    restore_result: Result<(), SourceError>,
) -> Result<(), SourceError> {
    let control_error = match control_result {
        Ok(error) => error,
        Err(error) => Some(error),
    };
    match (control_error, restore_result) {
        (None, Ok(())) => Ok(()),
        (Some(error), Ok(())) | (None, Err(error)) => Err(error),
        (Some(control), Err(restore)) => Err(SourceError::ControlAndRestore {
            control: Box::new(control),
            restore: Box::new(restore),
        }),
    }
}

fn create_source<'core>(
    core: &'core CoreRc,
    rate: Rate,
    dispatch: Dispatch,
    policy_route: Option<&str>,
) -> Result<CallbackSource<'core>, SourceError> {
    let process_id = std::process::id().to_string();
    let mut properties = properties! {
        "node.name" => "io.mixrelay.lab-application",
        "media.class" => "Stream/Output/Audio",
        "media.type" => "Audio", "media.category" => "Playback",
        "application.id" => "org.mixrelay.fixture.application",
        "application.process.id" => process_id.as_str(),
        "application.name" => "MixRelay Application Fixture",
        "node.autoconnect" => if policy_route.is_some() { "true" } else { "false" },
        "node.dont-fallback" => "true",
        "node.always-process" => "true", "node.pause-on-idle" => "false",
    };
    if let Some(target) = policy_route {
        // The private policy may create the owned endpoint after this stream.
        properties.insert("target.object", target);
        properties.insert("node.linger", "true");
    }
    let stream = StreamBox::new(core, "MixRelay application fixture", properties)?;
    let source = CallbackSource::new(stream, rate)?;
    let bytes = format_bytes(rate)?;
    let pod = Pod::from_bytes(&bytes).ok_or(SourceError::Format)?;
    let flags = dispatch.flags()
        | if policy_route.is_some() {
            StreamFlags::AUTOCONNECT
        } else {
            StreamFlags::empty()
        };
    source
        .stream()
        .connect(Direction::Output, None, flags, &mut [pod])?;
    Ok(source)
}

fn wait_for_node_id(
    source: &CallbackSource<'_>,
    loop_: &MainLoopRc,
    stopped: &Cell<bool>,
) -> Result<u32, SourceError> {
    let deadline = Instant::now() + MUTE_PARAM_TIMEOUT;
    loop {
        if stopped.get() {
            return Err(SourceError::Mute(
                "source stopped before PipeWire bound its node".into(),
            ));
        }
        match source.stream().state() {
            StreamState::Error(error) => {
                return Err(SourceError::Mute(format!(
                    "source failed before PipeWire bound its node: {error}"
                )));
            }
            StreamState::Unconnected => {
                return Err(SourceError::Mute(
                    "source became unconnected before PipeWire bound its node".into(),
                ));
            }
            StreamState::Connecting | StreamState::Paused | StreamState::Streaming => {}
        }
        let node_id = source.stream().node_id();
        if node_id != pipewire::spa::sys::SPA_ID_INVALID {
            return (node_id != 0)
                .then_some(node_id)
                .ok_or_else(|| SourceError::Mute("PipeWire bound source node id zero".into()));
        }
        if Instant::now() >= deadline {
            return Err(SourceError::Mute(
                "timed out waiting for PipeWire source node binding".into(),
            ));
        }
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
}

fn run_source_control(
    loop_: &MainLoopRc,
    stopped: &Cell<bool>,
    source: &CallbackSource<'_>,
    control: Option<&SourceMute>,
) -> Result<Option<SourceError>, SourceError> {
    let Some(control) = control else {
        while !stopped.get() {
            loop_
                .loop_()
                .iterate(Timeout::Finite(Duration::from_millis(20)));
        }
        return Ok(None);
    };
    let (_worker, receiver) = command_receiver();
    write_control_line(&serde_json::json!({
        "event": "ready",
        "nodeId": source.stream().node_id(),
        "nodeSerial": control.serial,
        "processId": std::process::id(),
        "applicationId": "org.mixrelay.fixture.application",
        "mute": control.original,
    }))?;
    run_control_commands(loop_, stopped, source, control, &receiver)
}

fn run_control_commands(
    loop_: &MainLoopRc,
    stopped: &Cell<bool>,
    source: &CallbackSource<'_>,
    control: &SourceMute,
    receiver: &Receiver<Result<String, String>>,
) -> Result<Option<SourceError>, SourceError> {
    let mut command_error = None;
    while !stopped.get() {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(20)));
        match receiver.try_recv() {
            Ok(Ok(command)) if command == "quit" => stopped.set(true),
            Ok(Ok(command)) => match handle_control_command(&command, source, control, loop_) {
                Ok(response) => write_control_line(&response)?,
                Err(error) => {
                    let _ = write_control_line(&serde_json::json!({
                        "event": "error",
                        "message": error.to_string(),
                    }));
                    command_error = Some(error);
                    stopped.set(true);
                }
            },
            Ok(Err(error)) => {
                command_error = Some(SourceError::Mute(error));
                stopped.set(true);
            }
            Err(TryRecvError::Disconnected) => stopped.set(true),
            Err(TryRecvError::Empty) => {}
        }
    }
    Ok(command_error)
}

fn handle_control_command(
    command: &str,
    source: &CallbackSource<'_>,
    control: &SourceMute,
    loop_: &MainLoopRc,
) -> Result<serde_json::Value, SourceError> {
    let mute = command
        .strip_prefix("set ")
        .and_then(|value| match value {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        })
        .ok_or_else(|| SourceError::Mute("invalid command".into()))?;
    control.set_and_confirm(mute, loop_, "capture-transition")?;
    Ok(serde_json::json!({
        "event": "mute",
        "mute": mute,
        "framePosition": source.frame_position(),
    }))
}

const MUTE_PARAM_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CORE_DIAGNOSTIC_EVENTS: usize = 16;

struct ParamReadback {
    attempt: Cell<u32>,
    operation: Cell<&'static str>,
    expected_seq: Cell<Option<i32>>,
    expected_proxy_id: Cell<u32>,
    expected_barrier: Cell<Option<i32>>,
    value: Cell<Option<bool>>,
    count: Cell<u8>,
    callback_count: Cell<u8>,
    invalid: Cell<bool>,
    barrier_seen: Cell<bool>,
    barrier_error: Cell<bool>,
    last_callback: Cell<Option<ParamReadbackEvent>>,
    core_error_count: Cell<u8>,
    set_param_result: Cell<Option<i32>>,
    core_events: RefCell<CoreEventHistory>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParamReadbackEvent {
    sequence: i32,
    param_type: u32,
    index: u32,
    next: u32,
    mute: Option<bool>,
    category: &'static str,
}

#[derive(Debug)]
enum CoreDiagnosticEvent {
    Done {
        attempt: u32,
        operation: &'static str,
        object_id: u32,
        sequence: i32,
    },
    Error {
        attempt: u32,
        operation: &'static str,
        object_id: u32,
        sequence: i32,
        code: i32,
        message: String,
    },
}

#[derive(Debug)]
struct CoreEventHistory {
    done_count: u32,
    error_count: u32,
    overflow_count: u32,
    events: VecDeque<CoreDiagnosticEvent>,
}

impl CoreEventHistory {
    fn new() -> Self {
        Self {
            done_count: 0,
            error_count: 0,
            overflow_count: 0,
            events: VecDeque::with_capacity(MAX_CORE_DIAGNOSTIC_EVENTS),
        }
    }

    fn record(&mut self, event: CoreDiagnosticEvent) {
        match &event {
            CoreDiagnosticEvent::Done { .. } => {
                self.done_count = self.done_count.saturating_add(1);
            }
            CoreDiagnosticEvent::Error { .. } => {
                self.error_count = self.error_count.saturating_add(1);
            }
        }
        if self.events.len() == MAX_CORE_DIAGNOSTIC_EVENTS {
            let _ = self.events.pop_front();
            self.overflow_count = self.overflow_count.saturating_add(1);
        }
        self.events.push_back(event);
    }

    fn diagnostic(&self) -> String {
        let events = self
            .events
            .iter()
            .map(CoreDiagnosticEvent::diagnostic)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "core_done_events={} core_error_events={} retained={} overflow={} events=[{}]",
            self.done_count,
            self.error_count,
            self.events.len(),
            self.overflow_count,
            events
        )
    }
}

impl CoreDiagnosticEvent {
    fn diagnostic(&self) -> String {
        match self {
            Self::Done {
                attempt,
                operation,
                object_id,
                sequence,
            } => format!("attempt={attempt}:{operation}:done(object={object_id},seq={sequence})"),
            Self::Error {
                attempt,
                operation,
                object_id,
                sequence,
                code,
                message,
            } => format!(
                "attempt={attempt}:{operation}:error(object={object_id},seq={sequence},code={code},message={message})"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MuteParamObservation {
    value: Option<bool>,
    category: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadbackStatus {
    Pending,
    Invalid,
    Value(bool),
}

impl ParamReadback {
    fn new() -> Self {
        Self {
            attempt: Cell::new(0),
            operation: Cell::new("unstarted"),
            expected_seq: Cell::new(None),
            expected_proxy_id: Cell::new(pipewire::core::PW_ID_CORE),
            expected_barrier: Cell::new(None),
            value: Cell::new(None),
            count: Cell::new(0),
            callback_count: Cell::new(0),
            invalid: Cell::new(false),
            barrier_seen: Cell::new(false),
            barrier_error: Cell::new(false),
            last_callback: Cell::new(None),
            core_error_count: Cell::new(0),
            set_param_result: Cell::new(None),
            core_events: RefCell::new(CoreEventHistory::new()),
        }
    }

    fn begin(&self, proxy_id: u32, operation: &'static str) {
        self.attempt.set(self.attempt.get().saturating_add(1));
        self.operation.set(operation);
        self.expected_seq.set(None);
        self.expected_proxy_id.set(proxy_id);
        self.expected_barrier.set(None);
        self.value.set(None);
        self.count.set(0);
        self.callback_count.set(0);
        self.invalid.set(false);
        self.barrier_seen.set(false);
        self.barrier_error.set(false);
        self.last_callback.set(None);
        self.core_error_count.set(0);
        self.set_param_result.set(None);
    }

    fn record_set_param_result(&self, result: i32) -> Result<(), SourceError> {
        self.set_param_result.set(Some(result));
        if result < 0 {
            return Err(SourceError::Mute(format!(
                "native node set_param failed ({})",
                self.diagnostic()
            )));
        }
        Ok(())
    }

    fn expect_request(&self, sequence: i32) {
        self.expected_seq.set(Some(sequence));
    }

    fn expect_barrier(&self, sequence: i32) {
        self.expected_barrier.set(Some(sequence));
    }

    fn observe_param(&self, event: ParamReadbackEvent) {
        self.callback_count
            .set(self.callback_count.get().saturating_add(1));
        self.last_callback.set(Some(event));
        if self.expected_seq.get() != Some(event.sequence) {
            return;
        }
        let count = self.count.get();
        self.count.set(count.saturating_add(1));
        if count != 0 || event.category != "parsed" || event.mute.is_none() {
            self.invalid.set(true);
            return;
        }
        self.value.set(event.mute);
    }

    fn observe_done(&self, object_id: u32, sequence: i32) {
        self.core_events
            .borrow_mut()
            .record(CoreDiagnosticEvent::Done {
                attempt: self.attempt.get(),
                operation: self.operation.get(),
                object_id,
                sequence,
            });
        if object_id == pipewire::core::PW_ID_CORE && self.expected_barrier.get() == Some(sequence)
        {
            self.barrier_seen.set(true);
        }
    }

    fn observe_error(&self, object_id: u32, sequence: i32, code: i32, message: &str) {
        self.core_events
            .borrow_mut()
            .record(CoreDiagnosticEvent::Error {
                attempt: self.attempt.get(),
                operation: self.operation.get(),
                object_id,
                sequence,
                code,
                message: diagnostic_property(Some(message)),
            });
        let matches_barrier = object_id == pipewire::core::PW_ID_CORE
            && self.expected_barrier.get() == Some(sequence);
        let matches_request =
            object_id == self.expected_proxy_id.get() && self.expected_seq.get() == Some(sequence);
        if matches_barrier {
            self.barrier_error.set(true);
        }
        if matches_barrier || matches_request {
            self.core_error_count
                .set(self.core_error_count.get().saturating_add(1));
        }
    }

    fn status(&self) -> ReadbackStatus {
        if self.core_error_count.get() != 0 {
            return ReadbackStatus::Invalid;
        }
        if !self.barrier_seen.get() {
            return ReadbackStatus::Pending;
        }
        if self.invalid.get() || self.count.get() != 1 {
            return ReadbackStatus::Invalid;
        }
        self.value
            .get()
            .map_or(ReadbackStatus::Invalid, ReadbackStatus::Value)
    }

    fn diagnostic(&self) -> String {
        let last_param = self.last_callback.get().map_or_else(
            || "none".to_owned(),
            |event| {
                let mute = match event.mute {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "unavailable",
                };
                format!(
                    "seq={} type={} index={} next={} mute={} category={}",
                    event.sequence, event.param_type, event.index, event.next, mute, event.category
                )
            },
        );
        let set_param_result = self
            .set_param_result
            .get()
            .map_or_else(|| "none".to_owned(), |result| result.to_string());
        let core_events = self.core_events.borrow().diagnostic();
        format!(
            "attempt={} operation={} set_param_result={} expected_proxy_id={} expected_seq={} expected_barrier={} callbacks={} matching_callbacks={} invalid={} barrier_seen={} barrier_error={} last_param=[{}] matching_core_errors={} {}",
            self.attempt.get(),
            self.operation.get(),
            set_param_result,
            self.expected_proxy_id.get(),
            self.expected_seq
                .get()
                .map_or_else(|| "none".to_owned(), |sequence| sequence.to_string()),
            self.expected_barrier
                .get()
                .map_or_else(|| "none".to_owned(), |sequence| sequence.to_string()),
            self.callback_count.get(),
            self.count.get(),
            self.invalid.get(),
            self.barrier_seen.get(),
            self.barrier_error.get(),
            last_param,
            self.core_error_count.get(),
            core_events
        )
    }
}

struct SourceMute {
    // Native hooks must unregister before their proxy/Core are destroyed. Node does not retain
    // CoreRc; retain the control connection until all listeners and the node proxy are gone.
    _listener: NodeListener,
    _identity_listener: NodeListener,
    _core_listener: CoreListener,
    _registry_listener: RegistryListener,
    node: Node,
    core: CoreRc,
    identity: Rc<RefCell<SourceIdentityObservation>>,
    removed: Rc<Cell<bool>>,
    node_id: u32,
    process_id: String,
    result: Rc<ParamReadback>,
    original: bool,
    serial: String,
}

struct BoundSourceNode {
    listener: RegistryListener,
    node: Node,
    removed: Rc<Cell<bool>>,
}

struct BoundSourceProxy {
    node: Node,
    removed: Rc<Cell<bool>>,
}

struct OwnedSourceNode {
    registry_listener: RegistryListener,
    identity_listener: NodeListener,
    node: Node,
    identity: Rc<RefCell<SourceIdentityObservation>>,
    removed: Rc<Cell<bool>>,
    serial: String,
}

enum SourceNodeBindingState {
    Waiting,
    Resolved(Result<BoundSourceProxy, String>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SourceNodeIdentity {
    node_name: Option<String>,
    media_class: Option<String>,
    application_id: Option<String>,
    process_id: Option<String>,
    serial: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourceIdentityObservation {
    Waiting(SourceNodeIdentity),
    Verified(String, SourceNodeIdentity),
    Rejected(String),
}

impl SourceNodeIdentity {
    fn from_info(info: &NodeInfoRef) -> Self {
        let properties = info.props();
        let property =
            |name| properties.and_then(|properties| properties.get(name).map(str::to_owned));
        Self {
            node_name: property("node.name"),
            media_class: property("media.class"),
            application_id: property("application.id"),
            process_id: property("application.process.id"),
            serial: property("object.serial"),
        }
    }

    fn conflicts_with_fixture(&self, process_id: &str) -> bool {
        self.node_name
            .as_deref()
            .is_some_and(|value| value != "io.mixrelay.lab-application")
            || self
                .media_class
                .as_deref()
                .is_some_and(|value| value != "Stream/Output/Audio")
            || self
                .application_id
                .as_deref()
                .is_some_and(|value| value != "org.mixrelay.fixture.application")
            || self
                .process_id
                .as_deref()
                .is_some_and(|value| value != process_id)
            || self.serial.as_deref() == Some("")
    }

    fn matches_fixture(&self, process_id: &str) -> bool {
        self.node_name.as_deref() == Some("io.mixrelay.lab-application")
            && self.media_class.as_deref() == Some("Stream/Output/Audio")
            && self.application_id.as_deref() == Some("org.mixrelay.fixture.application")
            && self.process_id.as_deref() == Some(process_id)
            && self
                .serial
                .as_deref()
                .is_some_and(|serial| !serial.is_empty())
    }

    fn diagnostic(&self, node_id: u32, process_id: &str) -> String {
        format!(
            "source node {node_id} identity did not verify (expected pid {process_id}; node.name={}, media.class={}, application.id={}, application.process.id={}, object.serial={})",
            diagnostic_property(self.node_name.as_deref()),
            diagnostic_property(self.media_class.as_deref()),
            diagnostic_property(self.application_id.as_deref()),
            diagnostic_property(self.process_id.as_deref()),
            diagnostic_property(self.serial.as_deref())
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

fn bind_owned_source_node(
    core: &CoreRc,
    loop_: &MainLoopRc,
    node_id: u32,
) -> Result<BoundSourceNode, SourceError> {
    if node_id == 0 {
        return Err(SourceError::Mute(
            "source stream has no native node id".into(),
        ));
    }
    let registry = core.get_registry_rc()?;
    let bound = Rc::new(RefCell::new(SourceNodeBindingState::Waiting));
    let callback_bound = Rc::clone(&bound);
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
            if !matches!(&*callback_bound.borrow(), SourceNodeBindingState::Waiting) {
                return;
            }
            let result = bind_registry
                .bind::<Node, _>(object)
                .map(|node| BoundSourceProxy {
                    node,
                    removed: Rc::clone(&callback_bind_removed),
                })
                .map_err(|error| format!("bind owned source node: {error}"));
            *callback_bound.borrow_mut() = SourceNodeBindingState::Resolved(result);
        })
        .global_remove(move |removed_id| {
            if removed_id == node_id {
                callback_remove_flag.set(true);
            }
        })
        .register();
    let proxy = wait_for_source_binding(loop_, &bound, &removed, node_id)?;
    Ok(BoundSourceNode {
        node: proxy.node,
        listener,
        removed: proxy.removed,
    })
}

fn wait_for_source_binding(
    loop_: &MainLoopRc,
    bound: &RefCell<SourceNodeBindingState>,
    removed: &Cell<bool>,
    node_id: u32,
) -> Result<BoundSourceProxy, SourceError> {
    let deadline = Instant::now() + MUTE_PARAM_TIMEOUT;
    while matches!(&*bound.borrow(), SourceNodeBindingState::Waiting)
        && !removed.get()
        && Instant::now() < deadline
    {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    let state = {
        let mut state = bound.borrow_mut();
        std::mem::replace(&mut *state, SourceNodeBindingState::Waiting)
    };
    match state {
        SourceNodeBindingState::Waiting if removed.get() => Err(SourceError::Mute(format!(
            "exact source node {node_id} was removed before binding"
        ))),
        SourceNodeBindingState::Waiting => Err(SourceError::Mute(format!(
            "timed out binding exact source node {node_id}"
        ))),
        SourceNodeBindingState::Resolved(Ok(node)) if node.removed.get() => Err(SourceError::Mute(
            format!("exact source node {node_id} was removed while binding"),
        )),
        SourceNodeBindingState::Resolved(Ok(node)) => Ok(node),
        SourceNodeBindingState::Resolved(Err(error)) => Err(SourceError::Mute(error)),
    }
}

fn verify_source_node(
    bound: BoundSourceNode,
    loop_: &MainLoopRc,
    node_id: u32,
    process_id: &str,
) -> Result<OwnedSourceNode, SourceError> {
    let identity = Rc::new(RefCell::new(SourceIdentityObservation::Waiting(
        SourceNodeIdentity::default(),
    )));
    let callback_identity = Rc::clone(&identity);
    let callback_process_id = process_id.to_owned();
    let listener = bound
        .node
        .add_listener_local()
        .info(move |info| {
            let previous = callback_identity.borrow().clone();
            let observed = observe_source_node_info(info, node_id, &callback_process_id, &previous);
            let mut state = callback_identity.borrow_mut();
            if !matches!(&*state, SourceIdentityObservation::Rejected(_)) {
                *state = observed;
            }
        })
        .register();
    let deadline = Instant::now() + MUTE_PARAM_TIMEOUT;
    while matches!(&*identity.borrow(), SourceIdentityObservation::Waiting(_))
        && !bound.removed.get()
        && Instant::now() < deadline
    {
        loop_
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(10)));
    }
    let observation = identity.borrow().clone();
    match observation {
        SourceIdentityObservation::Verified(serial, _) if !bound.removed.get() => {
            Ok(OwnedSourceNode {
                node: bound.node,
                registry_listener: bound.listener,
                identity_listener: listener,
                identity,
                removed: bound.removed,
                serial,
            })
        }
        SourceIdentityObservation::Rejected(error) => Err(SourceError::Mute(error)),
        SourceIdentityObservation::Verified(_, identity) => Err(SourceError::Mute(format!(
            "{}; native node was removed during identity verification",
            identity.diagnostic(node_id, process_id)
        ))),
        SourceIdentityObservation::Waiting(actual) => {
            Err(SourceError::Mute(if bound.removed.get() {
                format!(
                    "{}; native node was removed during identity verification",
                    actual.diagnostic(node_id, process_id)
                )
            } else {
                actual.diagnostic(node_id, process_id)
            }))
        }
    }
}

fn observe_source_node_info(
    info: &NodeInfoRef,
    expected_node_id: u32,
    process_id: &str,
    previous: &SourceIdentityObservation,
) -> SourceIdentityObservation {
    if info.id() != expected_node_id {
        let identity = match previous {
            SourceIdentityObservation::Waiting(identity)
            | SourceIdentityObservation::Verified(_, identity) => identity,
            SourceIdentityObservation::Rejected(error) => {
                return SourceIdentityObservation::Rejected(error.clone());
            }
        };
        return SourceIdentityObservation::Rejected(format!(
            "{}; native NodeInfo id was {}",
            identity.diagnostic(expected_node_id, process_id),
            info.id()
        ));
    }
    if info.change_mask().contains(NodeChangeMask::STATE)
        && let NodeState::Error(error) = info.state()
    {
        return reject_source_node_error(previous, expected_node_id, process_id, error);
    }
    let identity = info
        .change_mask()
        .contains(NodeChangeMask::PROPS)
        .then(|| SourceNodeIdentity::from_info(info));
    observe_source_properties(identity, process_id, expected_node_id, previous)
}

fn reject_source_node_error(
    previous: &SourceIdentityObservation,
    node_id: u32,
    process_id: &str,
    error: &str,
) -> SourceIdentityObservation {
    let identity = match previous {
        SourceIdentityObservation::Waiting(identity)
        | SourceIdentityObservation::Verified(_, identity) => identity,
        SourceIdentityObservation::Rejected(error) => {
            return SourceIdentityObservation::Rejected(error.clone());
        }
    };
    SourceIdentityObservation::Rejected(format!(
        "{}; native node error: {}",
        identity.diagnostic(node_id, process_id),
        diagnostic_property(Some(error))
    ))
}

fn observe_source_properties(
    actual: Option<SourceNodeIdentity>,
    process_id: &str,
    node_id: u32,
    previous: &SourceIdentityObservation,
) -> SourceIdentityObservation {
    let Some(identity) = actual else {
        return previous.clone();
    };
    if identity.conflicts_with_fixture(process_id) {
        return SourceIdentityObservation::Rejected(identity.diagnostic(node_id, process_id));
    }
    if let SourceIdentityObservation::Verified(serial, _) = previous
        && identity.serial.as_deref() != Some(serial.as_str())
    {
        return SourceIdentityObservation::Rejected(identity.diagnostic(node_id, process_id));
    }
    if identity.matches_fixture(process_id) {
        let serial = identity.serial.clone().unwrap_or_default();
        return SourceIdentityObservation::Verified(serial, identity);
    }
    if let SourceIdentityObservation::Verified(_, _) = previous {
        return SourceIdentityObservation::Rejected(identity.diagnostic(node_id, process_id));
    }
    SourceIdentityObservation::Waiting(identity)
}

fn attach_readback_listeners(
    node: &Node,
    core: &CoreRc,
) -> (Rc<ParamReadback>, NodeListener, CoreListener) {
    let result = Rc::new(ParamReadback::new());
    let callback_result = Rc::clone(&result);
    let node_listener = node
        .add_listener_local()
        .param(move |seq, param_type, index, next, param| {
            let observation = mute_value(param_type, param);
            callback_result.observe_param(ParamReadbackEvent {
                sequence: seq,
                param_type: param_type.as_raw(),
                index,
                next,
                mute: observation.value,
                category: observation.category,
            });
        })
        .register();
    let done_result = Rc::clone(&result);
    let error_result = Rc::clone(&result);
    let core_listener = core
        .add_listener_local()
        .done(move |object_id, sequence| {
            done_result.observe_done(object_id, sequence.seq());
        })
        .error(move |object_id, sequence, result, message| {
            error_result.observe_error(object_id, sequence, result, message);
        })
        .register();
    (result, node_listener, core_listener)
}

impl SourceMute {
    fn new(
        core: &pipewire::core::CoreRc,
        loop_: &MainLoopRc,
        node_id: u32,
    ) -> Result<Self, SourceError> {
        let bound = bind_owned_source_node(core, loop_, node_id)?;
        let process_id = std::process::id().to_string();
        let verified = verify_source_node(bound, loop_, node_id, &process_id)?;
        let (result, listener, core_listener) = attach_readback_listeners(&verified.node, core);
        let mut owner = Self {
            core: core.clone(),
            node: verified.node,
            _listener: listener,
            _identity_listener: verified.identity_listener,
            _core_listener: core_listener,
            _registry_listener: verified.registry_listener,
            identity: verified.identity,
            removed: verified.removed,
            node_id,
            process_id,
            result,
            original: false,
            serial: verified.serial,
        };
        owner.original = owner.read_back(loop_, "initial-readback")?;
        Ok(owner)
    }

    fn ensure_owned_identity(&self) -> Result<(), SourceError> {
        let observation = self.identity.borrow().clone();
        let message = match observation {
            SourceIdentityObservation::Verified(_, _) if !self.removed.get() => return Ok(()),
            SourceIdentityObservation::Verified(_, identity) => format!(
                "{}; native node was removed",
                identity.diagnostic(self.node_id, &self.process_id)
            ),
            SourceIdentityObservation::Waiting(identity) if self.removed.get() => format!(
                "{}; native node was removed",
                identity.diagnostic(self.node_id, &self.process_id)
            ),
            SourceIdentityObservation::Waiting(identity) => {
                identity.diagnostic(self.node_id, &self.process_id)
            }
            SourceIdentityObservation::Rejected(error) => error,
        };
        Err(SourceError::Mute(message))
    }

    fn request_props(&self) -> Result<i32, SourceError> {
        let proxy_id = self.node.upcast_ref().id();
        // SAFETY: `self.core` is the same live core used to bind `self.node`; the source owner
        // retains both objects on this non-Send control thread. The returned proxy is used only
        // in the next call and never escapes this borrowed owner.
        let proxy = unsafe { pipewire::sys::pw_core_find_proxy(self.core.as_raw_ptr(), proxy_id) };
        if proxy.is_null() {
            return Err(SourceError::Mute(format!(
                "native enum_params could not resolve owned node proxy {proxy_id}"
            )));
        }
        // SAFETY: `proxy` is the live proxy resolved from `self.node` through `self.core`; the
        // synchronous method call completes before this borrow can end or the loop is iterated.
        let raw_result = unsafe {
            pipewire::spa::spa_interface_call_method!(
                proxy,
                pipewire::sys::pw_node_methods,
                enum_params,
                0,
                ParamType::Props.as_raw(),
                0,
                u32::MAX,
                std::ptr::null()
            )
        };
        async_sequence(raw_result)
    }

    fn set_props(&self, param: &Pod) -> Result<i32, SourceError> {
        let proxy_id = self.node.upcast_ref().id();
        // SAFETY: `self.core` is the same live core used to bind `self.node`; the source owner
        // retains both objects and `param` on this non-Send control thread for the whole call.
        let proxy = unsafe { pipewire::sys::pw_core_find_proxy(self.core.as_raw_ptr(), proxy_id) };
        if proxy.is_null() {
            return Err(SourceError::Mute(format!(
                "native set_param could not resolve owned node proxy {proxy_id}"
            )));
        }
        // SAFETY: `proxy` remains owned by `self.node` and `param` lives through this synchronous
        // method call; neither pointer escapes or is retained by this fixture helper.
        let raw_result = unsafe {
            pipewire::spa::spa_interface_call_method!(
                proxy,
                pipewire::sys::pw_node_methods,
                set_param,
                ParamType::Props.as_raw(),
                0,
                param.as_raw_ptr()
            )
        };
        Ok(raw_result)
    }

    fn read_back(&self, loop_: &MainLoopRc, operation: &'static str) -> Result<bool, SourceError> {
        self.ensure_owned_identity()?;
        self.result.begin(self.node.upcast_ref().id(), operation);
        self.read_back_started(loop_)
    }

    fn read_back_started(&self, loop_: &MainLoopRc) -> Result<bool, SourceError> {
        self.ensure_owned_identity()?;
        let sequence = self.request_props()?;
        self.result.expect_request(sequence);
        let barrier = self.core.sync(0)?;
        self.result.expect_barrier(barrier.seq());
        let deadline = Instant::now() + MUTE_PARAM_TIMEOUT;
        while self.result.status() == ReadbackStatus::Pending && Instant::now() < deadline {
            loop_
                .loop_()
                .iterate(Timeout::Finite(Duration::from_millis(10)));
        }
        self.ensure_owned_identity()?;
        match self.result.status() {
            ReadbackStatus::Value(value) => Ok(value),
            ReadbackStatus::Invalid => Err(SourceError::Mute(format!(
                "mute readback was malformed, duplicated or failed ({})",
                self.result.diagnostic()
            ))),
            ReadbackStatus::Pending => Err(SourceError::Mute(format!(
                "timed out waiting for mute readback sync barrier ({})",
                self.result.diagnostic()
            ))),
        }
    }

    fn set_and_confirm(
        &self,
        mute: bool,
        loop_: &MainLoopRc,
        operation: &'static str,
    ) -> Result<(), SourceError> {
        self.ensure_owned_identity()?;
        self.result.begin(self.node.upcast_ref().id(), operation);
        let bytes = mute_pod(mute)?;
        let param = Pod::from_bytes(&bytes).ok_or(SourceError::Format)?;
        let result = self.set_props(param)?;
        self.result.record_set_param_result(result)?;
        let actual = self.read_back_started(loop_)?;
        if actual != mute {
            return Err(SourceError::Mute(format!(
                "requested mute={mute}, native readback={actual}"
            )));
        }
        Ok(())
    }
}

fn async_sequence(raw_result: i32) -> Result<i32, SourceError> {
    let success = SpaResult::from_c(raw_result)
        .into_result()
        .map_err(|error| SourceError::Mute(format!("native enum_params failed: {error}")))?;
    match success {
        SpaSuccess::Async(sequence) => Ok(sequence.raw()),
        SpaSuccess::Sync(value) => Err(SourceError::Mute(format!(
            "native enum_params returned synchronous result {value}; expected an async sequence"
        ))),
    }
}

fn mute_value(param_type: ParamType, param: Option<&Pod>) -> MuteParamObservation {
    if param_type != ParamType::Props {
        return MuteParamObservation {
            value: None,
            category: "unexpected-param-type",
        };
    }
    let Some(param) = param else {
        return MuteParamObservation {
            value: None,
            category: "missing-pod",
        };
    };
    let Ok(object) = param.as_object() else {
        return MuteParamObservation {
            value: None,
            category: "not-an-object-pod",
        };
    };
    if object.type_().as_raw() != pipewire::spa::sys::SPA_TYPE_OBJECT_Props {
        return MuteParamObservation {
            value: None,
            category: "wrong-object-type",
        };
    }
    if object.id().0 != ParamType::Props.as_raw() {
        return MuteParamObservation {
            value: None,
            category: "wrong-parameter-id",
        };
    }
    let Some(property) =
        object.find_prop(pipewire::spa::utils::Id(pipewire::spa::sys::SPA_PROP_mute))
    else {
        return MuteParamObservation {
            value: None,
            category: "missing-mute-property",
        };
    };
    match property.value().get_bool() {
        Ok(value) => MuteParamObservation {
            value: Some(value),
            category: "parsed",
        },
        Err(_) => MuteParamObservation {
            value: None,
            category: "mute-is-not-boolean",
        },
    }
}

fn mute_pod(mute: bool) -> Result<Vec<u8>, SourceError> {
    let value = Value::Object(Object {
        type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Props,
        id: ParamType::Props.as_raw(),
        properties: vec![Property::new(
            pipewire::spa::sys::SPA_PROP_mute,
            Value::Bool(mute),
        )],
    });
    PodSerializer::serialize(Cursor::new(Vec::new()), &value)
        .map(|(buffer, _)| buffer.into_inner())
        .map_err(|_| SourceError::Format)
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
        let length = available.iter().position(|byte| *byte == b'\n');
        let consumed = length.map_or(available.len(), |index| index + 1);
        let content = length.map_or(available, |index| &available[..index]);
        if bytes.len() + content.len() > MAX_CONTROL_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "control line exceeds 64 bytes",
            ));
        }
        bytes.extend_from_slice(content);
        reader.consume(consumed);
        if length.is_some() {
            return String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error));
        }
    }
}

fn write_control_line(value: &serde_json::Value) -> Result<(), SourceError> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_route_requires_one_exact_owned_render_name() {
        let exact = format!("io.mixrelay.endpoint.{}.render", "a".repeat(64));
        assert!(policy_route_target(&exact));
        assert!(!policy_route_target("io.mixrelay.endpoint.other.render"));
        assert!(!policy_route_target(&exact.replace(".render", ".capture")));
        assert!(!policy_route_target(&format!("{exact} trailing")));
    }

    fn begin_readback(readback: &ParamReadback, sequence: i32, proxy_id: u32) {
        readback.begin(proxy_id, "test-readback");
        readback.expect_request(sequence);
    }

    fn observe_mute(readback: &ParamReadback, sequence: i32, mute: bool) {
        readback.observe_param(ParamReadbackEvent {
            sequence,
            param_type: ParamType::Props.as_raw(),
            index: 0,
            next: 1,
            mute: Some(mute),
            category: "parsed",
        });
    }

    #[test]
    fn source_requires_its_exact_private_runtime_and_executable() {
        let executable = Path::new(
            "/workspace/.lab/mixrelay-integration-engine/target/native-tests/app-lab-abc123/application-fixture",
        );
        let runtime = executable.parent().unwrap().join("runtime");
        assert!(is_owned_lab(executable, Some(executable), Some(&runtime)));
        assert!(!is_owned_lab(executable, None, Some(&runtime)));
        assert!(!is_owned_lab(executable, Some(executable), None));
        assert!(!is_owned_lab(
            executable,
            Some(executable),
            Some(Path::new("/run/user/1000"))
        ));
        assert!(!is_owned_lab(
            Path::new("application-fixture"),
            Some(Path::new("application-fixture")),
            Some(Path::new("runtime"))
        ));
    }

    #[test]
    fn partial_registry_identity_is_not_a_mute_authorization() {
        let partial = SourceNodeIdentity {
            node_name: Some("io.mixrelay.lab-application".into()),
            media_class: Some("Stream/Output/Audio".into()),
            serial: Some("17".into()),
            ..SourceNodeIdentity::default()
        };
        assert!(!partial.conflicts_with_fixture("1234"));
        assert!(!partial.matches_fixture("1234"));
        assert!(
            partial
                .diagnostic(42, "1234")
                .contains("application.id=<missing>")
        );
    }

    #[test]
    fn full_node_info_identity_requires_exact_application_pid_and_serial() {
        let full = SourceNodeIdentity {
            node_name: Some("io.mixrelay.lab-application".into()),
            media_class: Some("Stream/Output/Audio".into()),
            application_id: Some("org.mixrelay.fixture.application".into()),
            process_id: Some("1234".into()),
            serial: Some("17".into()),
        };
        assert!(!full.conflicts_with_fixture("1234"));
        assert!(full.matches_fixture("1234"));

        let mut wrong_pid = full.clone();
        wrong_pid.process_id = Some("5678".into());
        assert!(wrong_pid.conflicts_with_fixture("1234"));
        assert!(!wrong_pid.matches_fixture("1234"));
    }

    #[test]
    fn source_identity_updates_wait_then_verify_and_fail_closed_on_replacement() {
        let partial = SourceNodeIdentity {
            node_name: Some("io.mixrelay.lab-application".into()),
            media_class: Some("Stream/Output/Audio".into()),
            serial: Some("17".into()),
            ..SourceNodeIdentity::default()
        };
        let waiting = observe_source_properties(
            Some(partial.clone()),
            "1234",
            42,
            &SourceIdentityObservation::Waiting(SourceNodeIdentity::default()),
        );
        assert_eq!(waiting, SourceIdentityObservation::Waiting(partial));

        let full = SourceNodeIdentity {
            node_name: Some("io.mixrelay.lab-application".into()),
            media_class: Some("Stream/Output/Audio".into()),
            application_id: Some("org.mixrelay.fixture.application".into()),
            process_id: Some("1234".into()),
            serial: Some("123".into()),
        };
        let verified = observe_source_properties(Some(full.clone()), "1234", 42, &waiting);
        assert_eq!(
            verified,
            SourceIdentityObservation::Verified("123".into(), full.clone())
        );
        assert_eq!(
            observe_source_properties(None, "1234", 42, &verified),
            verified,
            "state-only node info must preserve a verified properties snapshot"
        );

        let mut replacement = full.clone();
        replacement.process_id = Some("5678".into());
        assert!(matches!(
            observe_source_properties(Some(replacement), "1234", 42, &verified),
            SourceIdentityObservation::Rejected(_)
        ));

        let mut changed_serial = full;
        changed_serial.serial = Some("456".into());
        assert!(matches!(
            observe_source_properties(Some(changed_serial), "1234", 42, &verified),
            SourceIdentityObservation::Rejected(_)
        ));
    }

    #[test]
    fn identity_diagnostics_bound_each_property_value() {
        let value = "x".repeat(256);
        let diagnostic = diagnostic_property(Some(&value));
        assert!(diagnostic.chars().count() <= 100);
        assert!(diagnostic.ends_with("…\""));
    }

    #[test]
    fn control_and_restore_failures_are_both_reported() {
        let result = combine_control_and_restore(
            Ok(Some(SourceError::Mute("control failed".into()))),
            Err(SourceError::Mute("restore failed".into())),
        );
        let error = result.expect_err("both failures must be reported");
        assert_eq!(
            error.to_string(),
            "source control failed: application source mute fixture: control failed; \
             mute restoration failed: application source mute fixture: restore failed"
        );
    }

    #[test]
    fn unsupported_rates_fail_and_supported_pods_round_trip() {
        assert!(source_rate(Some("96000")).is_err());
        for value in [None, Some("44100"), Some("48000")] {
            let rate = source_rate(value).unwrap();
            let bytes = format_bytes(rate).unwrap();
            let mut parsed = AudioInfoRaw::new();
            parsed.parse(Pod::from_bytes(&bytes).unwrap()).unwrap();
            assert_eq!(parsed.rate(), u32::from(rate.hz()));
            assert_eq!(parsed.channels(), 2);
            assert_eq!(parsed.position()[..2], [3, 4]);
        }
    }

    #[test]
    fn mute_readback_waits_for_the_matching_ordered_core_barrier() {
        let readback = ParamReadback::new();
        begin_readback(&readback, 7, 42);
        readback.expect_barrier(12);
        observe_mute(&readback, 6, true);
        readback.observe_done(pipewire::core::PW_ID_CORE, 11);
        assert_eq!(readback.status(), ReadbackStatus::Pending);
        observe_mute(&readback, 7, false);
        assert_eq!(readback.status(), ReadbackStatus::Pending);
        readback.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(readback.status(), ReadbackStatus::Value(false));
        let diagnostic = readback.diagnostic();
        assert!(diagnostic.contains("expected_seq=7"));
        assert!(diagnostic.contains("expected_proxy_id=42 expected_seq=7 expected_barrier=12"));
        assert!(diagnostic.contains("callbacks=2 matching_callbacks=1"));
        let expected_props = format!(
            "last_param=[seq=7 type={} index=0 next=1 mute=false category=parsed]",
            ParamType::Props.as_raw()
        );
        assert!(diagnostic.contains(&expected_props));
    }

    #[test]
    fn mute_readback_rejects_incomplete_or_duplicate_props_results() {
        let incomplete = ParamReadback::new();
        begin_readback(&incomplete, 3, 42);
        incomplete.expect_barrier(9);
        incomplete.observe_done(pipewire::core::PW_ID_CORE, 9);
        assert_eq!(incomplete.status(), ReadbackStatus::Invalid);
        assert!(
            incomplete
                .diagnostic()
                .contains("callbacks=0 matching_callbacks=0")
        );

        let wrong_sequence = ParamReadback::new();
        begin_readback(&wrong_sequence, 8, 42);
        wrong_sequence.expect_barrier(12);
        observe_mute(&wrong_sequence, 7, false);
        wrong_sequence.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(wrong_sequence.status(), ReadbackStatus::Invalid);
        let diagnostic = wrong_sequence.diagnostic();
        assert!(diagnostic.contains("expected_seq=8"));
        assert!(diagnostic.contains("callbacks=1 matching_callbacks=0"));
        assert!(diagnostic.contains("last_param=[seq=7"));

        let duplicate = ParamReadback::new();
        begin_readback(&duplicate, 4, 42);
        duplicate.expect_barrier(10);
        observe_mute(&duplicate, 4, false);
        observe_mute(&duplicate, 4, true);
        duplicate.observe_done(pipewire::core::PW_ID_CORE, 10);
        assert_eq!(duplicate.status(), ReadbackStatus::Invalid);
        assert!(duplicate.diagnostic().contains("matching_callbacks=2"));
    }

    #[test]
    fn mute_readback_diagnostic_exposes_malformed_param_and_bounded_core_error() {
        let readback = ParamReadback::new();
        begin_readback(&readback, 5, 202);
        readback.expect_barrier(11);
        readback.observe_param(ParamReadbackEvent {
            sequence: 5,
            param_type: ParamType::Props.as_raw(),
            index: 2,
            next: 3,
            mute: None,
            category: "mute-is-not-boolean",
        });
        readback.observe_error(42, 5, -22, "global node id is not the proxy id");
        assert!(readback.diagnostic().contains("matching_core_errors=0"));
        readback.observe_error(202, 5, -22, &"native enumeration rejected".repeat(16));
        readback.observe_done(pipewire::core::PW_ID_CORE, 11);
        assert_eq!(readback.status(), ReadbackStatus::Invalid);
        let diagnostic = readback.diagnostic();
        let expected_props = format!(
            "last_param=[seq=5 type={} index=2 next=3 mute=unavailable category=mute-is-not-boolean]",
            ParamType::Props.as_raw()
        );
        assert!(diagnostic.contains(&expected_props));
        assert!(diagnostic.contains("object=202,seq=5,code=-22"));
        assert!(diagnostic.contains("native enumeration rejected"));
        assert!(diagnostic.chars().count() < 900);
    }

    #[test]
    fn mute_readback_rejects_wrong_core_barriers_and_sync_errors() {
        let readback = ParamReadback::new();
        begin_readback(&readback, 5, 42);
        readback.expect_barrier(11);
        observe_mute(&readback, 5, false);
        readback.observe_done(pipewire::core::PW_ID_CORE + 1, 11);
        readback.observe_error(pipewire::core::PW_ID_CORE, 10, -1, "unmatched");
        assert_eq!(readback.status(), ReadbackStatus::Pending);
        readback.observe_error(pipewire::core::PW_ID_CORE, 11, -5, "sync failed");
        assert_eq!(readback.status(), ReadbackStatus::Invalid);
        let core_error = format!("object={},seq=11,code=-5", pipewire::core::PW_ID_CORE);
        assert!(readback.diagnostic().contains(&core_error));
    }

    #[test]
    fn enum_params_uses_exact_native_async_sequence_and_rejects_other_results() {
        let returned = pipewire::spa::utils::result::AsyncSeq::from_seq(1);
        assert_ne!(
            returned.raw(),
            1,
            "native async result carries the async bit"
        );
        assert_eq!(async_sequence(returned.raw()).unwrap(), returned.raw());
        assert!(
            async_sequence(1).is_err(),
            "synchronous success is not a reply sequence"
        );
        assert!(
            async_sequence(-22).is_err(),
            "native method errors stay failures"
        );

        let readback = ParamReadback::new();
        begin_readback(&readback, returned.raw(), 42);
        readback.expect_barrier(12);
        observe_mute(&readback, 1, false);
        readback.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(readback.status(), ReadbackStatus::Invalid);
        assert!(
            readback
                .diagnostic()
                .contains(&format!("expected_seq={}", returned.raw()))
        );
        assert!(readback.diagnostic().contains("last_param=[seq=1"));

        let matching = ParamReadback::new();
        begin_readback(&matching, returned.raw(), 42);
        matching.expect_barrier(12);
        observe_mute(&matching, returned.raw(), false);
        matching.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(matching.status(), ReadbackStatus::Value(false));
    }

    #[test]
    fn enum_error_cannot_be_hidden_by_a_valid_partial_reply() {
        let readback = ParamReadback::new();
        let sequence = pipewire::spa::utils::result::AsyncSeq::from_seq(8).raw();
        begin_readback(&readback, sequence, 202);
        readback.expect_barrier(12);
        observe_mute(&readback, sequence, false);
        readback.observe_error(202, sequence - 1, -22, "unrelated request");
        assert_eq!(readback.status(), ReadbackStatus::Pending);
        readback.observe_error(202, sequence, -22, "enumeration failed after partial reply");
        assert_eq!(readback.status(), ReadbackStatus::Invalid);
        readback.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(readback.status(), ReadbackStatus::Invalid);
    }

    #[test]
    fn unmatched_core_events_are_retained_but_never_satisfy_the_readback() {
        let readback = ParamReadback::new();
        begin_readback(&readback, 7, 42);
        readback.expect_barrier(12);
        readback.observe_error(88, 6, -32, "unmatched native error");
        readback.observe_done(pipewire::core::PW_ID_CORE + 1, 12);
        observe_mute(&readback, 7, false);
        assert_eq!(readback.status(), ReadbackStatus::Pending);
        let pending = readback.diagnostic();
        assert!(pending.contains("core_done_events=1 core_error_events=1 retained=2 overflow=0"));
        assert!(
            pending.contains("error(object=88,seq=6,code=-32,message=\"unmatched native error\")")
        );
        assert!(pending.contains("done(object=1,seq=12)"));

        readback.observe_done(pipewire::core::PW_ID_CORE, 12);
        assert_eq!(readback.status(), ReadbackStatus::Value(false));
        readback.begin(42, "restore-original");
        let restored = readback.diagnostic();
        assert!(restored.contains("attempt=1:test-readback:error(object=88"));
        assert!(restored.contains("attempt=1:test-readback:done(object=1,seq=12)"));
        assert!(restored.contains("attempt=2 operation=restore-original"));
    }

    #[test]
    fn node_set_param_records_and_rejects_negative_native_results() {
        let accepted = ParamReadback::new();
        accepted.begin(42, "capture-transition");
        assert!(accepted.record_set_param_result(0).is_ok());
        assert!(accepted.diagnostic().contains("set_param_result=0"));

        let asynchronous = ParamReadback::new();
        asynchronous.begin(42, "capture-transition");
        let async_result = pipewire::spa::utils::result::AsyncSeq::from_seq(8).raw();
        assert!(asynchronous.record_set_param_result(async_result).is_ok());
        assert!(
            asynchronous
                .diagnostic()
                .contains(&format!("set_param_result={async_result}"))
        );
        assert_eq!(asynchronous.status(), ReadbackStatus::Pending);

        let rejected = ParamReadback::new();
        rejected.begin(42, "capture-transition");
        let error = rejected
            .record_set_param_result(-13)
            .expect_err("a negative native method return must fail closed");
        assert!(error.to_string().contains("native node set_param failed"));
        assert!(error.to_string().contains("set_param_result=-13"));
    }

    #[test]
    fn core_event_history_is_bounded_reports_overflow_and_sanitizes_long_utf8() {
        let readback = ParamReadback::new();
        begin_readback(&readback, 7, 42);
        readback.expect_barrier(99);
        let message = "音声状態".repeat(64);
        let event_count = i32::try_from(MAX_CORE_DIAGNOSTIC_EVENTS + 2)
            .expect("the fixed diagnostic test bound fits i32");
        for sequence in 0..event_count {
            readback.observe_error(88, sequence, -32, &message);
        }
        let diagnostic = readback.diagnostic();
        assert!(diagnostic.contains(&format!(
            "core_done_events=0 core_error_events={} retained={} overflow=2",
            MAX_CORE_DIAGNOSTIC_EVENTS + 2,
            MAX_CORE_DIAGNOSTIC_EVENTS
        )));
        assert!(diagnostic.contains("message=\"????????"));
        assert!(diagnostic.chars().count() < 4_000);
        assert_eq!(readback.status(), ReadbackStatus::Pending);
    }
}
