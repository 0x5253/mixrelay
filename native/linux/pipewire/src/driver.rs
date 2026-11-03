//! Unique graph-filter ownership; only the native callback can borrow its processor.
use crate::control_time::{ControlTime, Observation, WaitError, wait_for};
use crate::{Connection, ConnectionError};
use pipewire::{properties::properties, spa, sys};
use std::{
    cell::UnsafeCell,
    collections::BTreeMap,
    ffi::{c_char, c_void},
    ptr::NonNull,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

mod failure;
use failure::FailurePublication;

#[cfg(test)]
mod tests;

/// Actual server clock position for one bounded native processing invocation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverCycle {
    /// Absolute server sample position.
    pub position: u64,
    /// Server-provided monotonic timestamp in nanoseconds, not callback work time.
    pub monotonic_ns: u64,
    /// Identity of the native clock, independent of its nominal rate.
    pub clock_id: u32,
    /// Native clock rate in frames per second.
    pub rate: u32,
    /// Frames in this native cycle, not the application DSP quantum.
    pub frames: u32,
}

impl DriverCycle {
    /// Upper bound on native callback work; independent of the requested DSP block.
    pub const MAX_FRAMES: u32 = 32_768;

    fn from_position(position: &spa::sys::spa_io_position) -> Option<Self> {
        let clock = &position.clock;
        if clock.rate.num != 1 || !(8_000..=384_000).contains(&clock.rate.denom) {
            return None;
        }
        let frames = u32::try_from(clock.duration)
            .ok()
            .filter(|frames| (1..=Self::MAX_FRAMES).contains(frames))?;
        Some(Self {
            position: clock.position,
            monotonic_ns: clock.nsec,
            clock_id: clock.id,
            rate: clock.rate.denom,
            frames,
        })
    }
}

/// Callback-owned processor moved from control to the serialized native data thread.
/// Implementations must not allocate, block, log or panic during `process`.
/// Borrowed fields remain valid because [`GraphDriver`] drains before dropping this value.
pub trait DriverProcess: Send {
    /// Prepares capture dependencies on control before publishing the processor.
    /// The native filter is active for negotiation, but no callback can borrow
    /// this processor yet. A failure prevents DSP publication.
    /// The default is for clock-only probes with no capture owners.
    /// # Errors
    /// Return a setup error if any required owner cannot bind before publication.
    fn prepare_inputs(&mut self, _inputs: &GraphInputs) -> Result<(), DriverError> {
        Ok(())
    }
    /// Processes the server's current clock position; the borrowed cycle cannot escape.
    fn process(&mut self, cycle: &DriverCycle);

    /// Observes this processor's first terminal fault on its serialized callback.
    /// The driver publishes it atomically for control and blocks further callbacks.
    fn failure(&self) -> Option<DriverProcessFailure> {
        None
    }
}

/// Initial scheduling preference of this processing owner, not server policy.
/// The server may select a different rate or quantum; callbacks always report
/// the actual clock through [`DriverCycle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverFormat {
    sample_rate: u32,
    block_frames: u32,
}

impl DriverFormat {
    /// Validates a prepared owner's nominal rate and processing block size.
    /// # Errors
    /// Returns [`DriverError::InvalidFormat`] outside 8–384 kHz or 1–8192 frames.
    pub fn new(sample_rate: u32, block_frames: u32) -> Result<Self, DriverError> {
        if !(8_000..=384_000).contains(&sample_rate) || !(1..=8192).contains(&block_frames) {
            return Err(DriverError::InvalidFormat);
        }
        Ok(Self {
            sample_rate,
            block_frames,
        })
    }

    fn properties(self) -> pipewire::properties::PropertiesBox {
        properties! {
            "node.name" => "io.mixrelay.graph", "media.type" => "Audio",
            "media.category" => "Filter", "media.role" => "DSP",
            "node.group" => "mixrelay.graph",
            "node.rate" => format!("1/{}", self.sample_rate),
            "node.latency" => format!("{}/{}", self.block_frames, self.sample_rate),
            "node.always-process" => "true", "node.want-driver" => "true",
        }
    }
}

/// Failure to prepare or start the graph clock, with all earlier stages reclaimed.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    /// The requested owner format is outside supported processing bounds.
    #[error("invalid PipeWire graph driver format")]
    InvalidFormat,
    /// Native context or its control thread failed.
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    /// Native filter allocation failed before registration.
    #[error("PipeWire graph filter allocation failed")]
    Create,
    /// Native connection rejected the filter, preserving its negative errno.
    #[error("PipeWire graph filter connection failed: {0}")]
    Connect(i32),
    /// Capture dependencies could not be negotiated before DSP publication.
    #[error("PipeWire capture dependency: {0}")]
    Input(String),
}

/// Terminal callback failures retained for control-side reporting after native drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverProcessFailure {
    MissingPosition,
    InvalidPosition,
    DependencyQueue(i32),
    FrameBudget {
        native_frames: u32,
        native_rate: u32,
        stream_rate: u32,
        capacity_frames: u32,
    },
}

struct InputLink {
    // Remove the listener before releasing its proxy, under the connection lock.
    _listener: pipewire::link::LinkListener,
    _link: pipewire::link::Link,
}

struct InputControl {
    connection: Connection,
    filter: Option<NonNull<sys::pw_filter>>,
    links: BTreeMap<u64, InputLink>,
    next_link: u64,
}

// SAFETY: only control accesses these handles, through the mutex and native loop
// lock. Link listeners capture only Arc<AtomicI32>, never thread-local Rc data.
// The RT callback has separate pinned storage and never borrows this owner.
unsafe impl Send for InputControl {}

/// Non-owning control access to this driver's capture dependency port.
/// It cannot borrow PCM, revive a closed driver or perform work on RT.
#[derive(Clone)]
pub struct GraphInputs(Weak<Mutex<InputControl>>);

/// One capture owner's negotiated native dependency link.
/// Keep it with that owner; retire it on control before closing its capture stream.
pub struct GraphInputBinding {
    owner: Weak<Mutex<InputControl>>,
    key: u64,
}

impl Drop for GraphInputBinding {
    fn drop(&mut self) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let mut control = owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let InputControl {
            connection, links, ..
        } = &mut *control;
        let _guard = connection.lock();
        links.remove(&self.key);
    }
}

impl GraphInputs {
    /// Checks control-side liveness without retaining or reviving the driver.
    /// # Errors
    /// Returns an error once teardown has closed the native filter.
    pub fn ensure_live(&self) -> Result<(), DriverError> {
        let owner = self
            .0
            .upgrade()
            .ok_or_else(|| DriverError::Input("driver closed".into()))?;
        let control = owner
            .lock()
            .map_err(|_| DriverError::Input("control poisoned".into()))?;
        if control.filter.is_none() {
            return Err(DriverError::Input("driver closed".into()));
        }
        Ok(())
    }
    /// Links an already exported capture adapter's monitor output to this driver.
    /// The adapter remains the sole RAW PCM converter. `monitor` names its first
    /// DSP monitor port; these samples establish ordering, never enter the DSP.
    /// # Errors
    /// Returns setup failure if the driver closed or negotiation did not complete.
    pub fn bind(&self, node_id: u32, monitor: &str) -> Result<GraphInputBinding, DriverError> {
        let owner = self
            .0
            .upgrade()
            .ok_or_else(|| DriverError::Input("driver closed".into()))?;
        let (key, state) = {
            let mut control = owner
                .lock()
                .map_err(|_| DriverError::Input("control poisoned".into()))?;
            control.create_link(node_id, monitor)?
        };
        let binding = GraphInputBinding {
            owner: self.0.clone(),
            key,
        };
        let clock = mixrelay_time::StdClock::new();
        wait_for(&clock, || match state.load(Ordering::Acquire) {
            sys::pw_link_state_PW_LINK_STATE_PAUSED | sys::pw_link_state_PW_LINK_STATE_ACTIVE => {
                Observation::Ready(())
            }
            sys::pw_link_state_PW_LINK_STATE_ERROR => Observation::Failed,
            _ => Observation::Pending,
        })
        .map_err(|error| {
            DriverError::Input(
                match error {
                    WaitError::Failed => "link negotiation failed",
                    WaitError::TimedOut => "link negotiation timed out",
                }
                .into(),
            )
        })?;
        self.ensure_live()?;
        Ok(binding)
    }
}

impl InputControl {
    fn create_link(
        &mut self,
        node_id: u32,
        monitor: &str,
    ) -> Result<(u64, Arc<AtomicI32>), DriverError> {
        let filter = self
            .filter
            .ok_or_else(|| DriverError::Input("driver closed".into()))?;
        let _guard = self.connection.lock();
        // SAFETY: filter remains owned here, and control holds its native loop lock.
        let input_id = unsafe { sys::pw_filter_get_node_id(filter.as_ptr()) };
        if input_id == sys::PW_ID_ANY {
            return Err(DriverError::Input("driver is not exported".into()));
        }
        let properties = properties! {
            "link.output.node" => node_id.to_string(), "link.output.port" => monitor,
            "link.input.node" => input_id.to_string(), "link.input.port" => "capture_dependency",
            "object.linger" => "false",
        };
        let link: pipewire::link::Link = self
            .connection
            .core()
            .create_object_cstr(c"link-factory", &properties)
            .map_err(|error| DriverError::Input(error.to_string()))?;
        let state = Arc::new(AtomicI32::new(sys::pw_link_state_PW_LINK_STATE_INIT));
        let observed = Arc::clone(&state);
        let listener = link
            .add_listener_local()
            .info(move |info| {
                use pipewire::link::LinkState;
                let native = match info.state() {
                    LinkState::Paused => sys::pw_link_state_PW_LINK_STATE_PAUSED,
                    LinkState::Active => sys::pw_link_state_PW_LINK_STATE_ACTIVE,
                    LinkState::Error(_) | LinkState::Unlinked => {
                        sys::pw_link_state_PW_LINK_STATE_ERROR
                    }
                    _ => sys::pw_link_state_PW_LINK_STATE_INIT,
                };
                observed.store(native, Ordering::Release);
            })
            .register();
        self.next_link += 1;
        let key = self.next_link;
        self.links.insert(
            key,
            InputLink {
                _listener: listener,
                _link: link,
            },
        );
        Ok((key, state))
    }
}

struct Callbacks<P> {
    state: AtomicI32,
    // One-way control-to-RT ownership publication, not per-cycle readiness.
    published: AtomicBool,
    failure: FailurePublication,
    // Main-loop events touch only state. Control prepares this cell before
    // publication; afterwards only RT_PROCESS borrows it.
    process: UnsafeCell<P>,
    input_port: *mut c_void,
}

/// Owns processor, native filter/listener and the private connection together.
/// Construction and destruction run on control, never on the audio callback.
/// Initial control preparation precedes DSP publication; afterwards the supervising
/// thread cannot access the processor until native teardown drains callbacks.
///
/// Borrowed processing state stays exclusively borrowed through driver teardown:
/// ```compile_fail
/// use mixrelay_linux_pipewire::{DriverCycle, DriverFormat, DriverProcess, GraphDriver};
/// struct Counter<'a>(&'a mut u64);
/// impl DriverProcess for Counter<'_> {
///     fn process(&mut self, _: &DriverCycle) { *self.0 += 1; }
/// }
/// let mut count = 0;
/// let format = DriverFormat::new(48_000, 128).unwrap();
/// let driver = GraphDriver::open(Counter(&mut count), format);
/// count = 10; // The callback still owns this mutable borrow.
/// drop(driver);
/// ```
pub struct GraphDriver<P: DriverProcess> {
    // After Drop disconnects/destroys the filter, release core/context/data loop
    // before callback storage. No callback or borrowed endpoint can survive it.
    control: Arc<Mutex<InputControl>>,
    listener: Box<spa::sys::spa_hook>,
    events: Box<sys::pw_filter_events>,
    callbacks: Box<Callbacks<P>>,
}

// SAFETY: moving this unique control owner does not move its boxed listener,
// events or processor addresses retained by PipeWire. Native callbacks borrow
// only those stable boxes on the serialized data loop; ready uses an atomic.
// All control/native destruction is loop-locked, and Drop drains callbacks
// before releasing the boxes and processor. No Sync or shared PCM access exists.
unsafe impl<P: DriverProcess> Send for GraphDriver<P> {}

impl<P: DriverProcess> GraphDriver<P> {
    /// Prepares and starts one graph filter, taking exclusive ownership of `process`.
    /// # Errors
    /// Returns the failed stage and releases the processor off RT after native drain.
    pub fn open(process: P, format: DriverFormat) -> Result<Self, DriverError> {
        let connection = Connection::new(c"mixrelay-clock")?;
        let properties = format.properties();
        // SAFETY: unique unstarted connection; native takes properties even on
        // failure. No Rust handle clone escapes into the callbacks.
        let filter = NonNull::new(unsafe {
            sys::pw_filter_new(
                connection.core().as_raw_ptr(),
                c"MixRelay graph".as_ptr(),
                properties.into_raw(),
            )
        })
        .ok_or(DriverError::Create)?;
        // SAFETY: spa_hook's C registration contract starts with a zeroed hook;
        // all fields are native pointers/callback options or their POD containers.
        let listener = Box::new(unsafe { std::mem::zeroed() });
        let mut driver = Self {
            control: Arc::new(Mutex::new(InputControl {
                connection,
                filter: Some(filter),
                links: BTreeMap::new(),
                next_link: 0,
            })),
            listener,
            events: Box::new(events::<P>()),
            callbacks: Box::new(Callbacks {
                state: AtomicI32::new(sys::pw_filter_state_PW_FILTER_STATE_UNCONNECTED),
                published: AtomicBool::new(false),
                failure: FailurePublication::default(),
                process: UnsafeCell::new(process),
                input_port: std::ptr::null_mut(),
            }),
        };
        driver.connect(filter)?;
        let clock = mixrelay_time::StdClock::new();
        driver.start(filter, &clock)?;
        Ok(driver)
    }

    fn connect(&mut self, filter: NonNull<sys::pw_filter>) -> Result<(), DriverError> {
        // SAFETY: boxed storage is pinned, this owner reclaims failed setup and
        // the connection has not started its control or RT dispatch yet.
        let result = unsafe {
            sys::pw_filter_add_listener(
                filter.as_ptr(),
                &raw mut *self.listener,
                &raw const *self.events,
                (&raw const *self.callbacks).cast_mut().cast(),
            );
            let properties = properties! {
                "port.name" => "capture_dependency", "format.dsp" => "32 bit float mono audio",
            };
            self.callbacks.input_port = sys::pw_filter_add_port(
                filter.as_ptr(),
                spa::utils::Direction::Input.as_raw(),
                sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
                0,
                properties.into_raw(),
                std::ptr::null_mut(),
                0,
            );
            if self.callbacks.input_port.is_null() {
                return Err(DriverError::Input(
                    "could not create dependency port".into(),
                ));
            }
            sys::pw_filter_connect(
                filter.as_ptr(),
                sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS
                    | sys::pw_filter_flags_PW_FILTER_FLAG_INACTIVE,
                std::ptr::null_mut(),
                0,
            )
        };
        if result < 0 {
            return Err(DriverError::Connect(result));
        }
        Ok(())
    }

    fn start(
        &mut self,
        filter: NonNull<sys::pw_filter>,
        clock: &dyn ControlTime,
    ) -> Result<(), DriverError> {
        {
            let mut control = self
                .control
                .lock()
                .map_err(|_| DriverError::Input("control poisoned".into()))?;
            control.connection.start()?;
        }
        wait_for(clock, || {
            match self.callbacks.state.load(Ordering::Acquire) {
                sys::pw_filter_state_PW_FILTER_STATE_PAUSED => Observation::Ready(()),
                sys::pw_filter_state_PW_FILTER_STATE_ERROR => Observation::Failed,
                _ => Observation::Pending,
            }
        })
        .map_err(|error| {
            DriverError::Input(
                match error {
                    WaitError::Failed => "driver export failed",
                    WaitError::TimedOut => "driver export timed out",
                }
                .into(),
            )
        })?;
        {
            let control = self
                .control
                .lock()
                .map_err(|_| DriverError::Input("control poisoned".into()))?;
            let _guard = control.connection.lock();
            // SAFETY: this owner retains the filter and native loop lock. PipeWire
            // prepares links only when both nodes are active; DSP remains unpublished.
            let result = unsafe { sys::pw_filter_set_active(filter.as_ptr(), true) };
            if result < 0 {
                return Err(DriverError::Connect(result));
            }
        }
        let inputs = GraphInputs(Arc::downgrade(&self.control));
        // SAFETY: the gate is still closed, so callbacks cannot borrow this cell.
        // Use its raw pointee, not a mutable reference to shared callback storage.
        unsafe { &mut *self.callbacks.process.get() }.prepare_inputs(&inputs)?;
        self.callbacks.published.store(true, Ordering::Release);
        Ok(())
    }

    /// True only while native streaming and the processor's terminal gate are healthy.
    #[must_use]
    pub fn ready(&self) -> bool {
        !self.callbacks.failure.is_set()
            && self.callbacks.state.load(Ordering::Acquire)
                == sys::pw_filter_state_PW_FILTER_STATE_STREAMING
    }

    /// Reports terminal native or processor failure, excluding pause and negotiation.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.callbacks.failure.is_set()
            || matches!(
                self.callbacks.state.load(Ordering::Acquire),
                sys::pw_filter_state_PW_FILTER_STATE_UNCONNECTED
                    | sys::pw_filter_state_PW_FILTER_STATE_ERROR
            )
    }

    /// Reads the first callback failure without clearing its terminal gate.
    #[must_use]
    pub fn process_failure(&self) -> Option<DriverProcessFailure> {
        self.callbacks.failure.read()
    }

    /// Drains native callbacks before reading their final failure cause on control.
    #[must_use]
    pub fn finish(mut self) -> Option<DriverProcessFailure> {
        self.disconnect();
        self.process_failure()
    }
}

impl<P: DriverProcess> GraphDriver<P> {
    fn disconnect(&mut self) {
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let InputControl {
            connection,
            filter,
            links,
            ..
        } = &mut *control;
        let _guard = connection.lock();
        // SAFETY: exclusive filter owner, main callbacks excluded by this lock.
        // Disconnect removes the exported node; destroy releases the native
        // listener before our boxed events/processor and parent context are freed.
        unsafe {
            if let Some(filter) = filter.take() {
                sys::pw_filter_disconnect(filter.as_ptr());
                sys::pw_filter_destroy(filter.as_ptr());
            }
        }
        // Native filter destruction drains DSP before links and processor-owned captures retire.
        links.clear();
    }
}

impl<P: DriverProcess> Drop for GraphDriver<P> {
    fn drop(&mut self) {
        self.disconnect();
    }
}

fn events<P: DriverProcess>() -> sys::pw_filter_events {
    sys::pw_filter_events {
        version: sys::PW_VERSION_FILTER_EVENTS,
        destroy: None,
        state_changed: Some(state_changed::<P>),
        io_changed: None,
        param_changed: None,
        add_buffer: None,
        remove_buffer: None,
        process: Some(process::<P>),
        drained: None,
        command: None,
    }
}

unsafe extern "C" fn state_changed<P: DriverProcess>(
    data: *mut c_void,
    _old: sys::pw_filter_state,
    state: sys::pw_filter_state,
    _error: *const c_char,
) {
    // SAFETY: GraphDriver retains this box until filter destruction/control drain.
    // This callback shares only the atomic, never the RT-owned process cell.
    let callbacks = unsafe { &*data.cast::<Callbacks<P>>() };
    callbacks.state.store(state, Ordering::Release);
}

unsafe extern "C" fn process<P: DriverProcess>(
    data: *mut c_void,
    position: *mut spa::sys::spa_io_position,
) {
    // SAFETY: GraphDriver pins this storage through callback drain.
    let callbacks = unsafe { &*data.cast::<Callbacks<P>>() };
    // Only serialized RT callbacks write this complete atomic payload.
    if callbacks.failure.is_set() {
        return;
    }
    if !callbacks.input_port.is_null() {
        // SAFETY: the pinned input port outlives callbacks; these two operations
        // are RT-safe. Recycle the dependency PCM without adding it to the DSP.
        unsafe {
            let buffer = sys::pw_filter_dequeue_buffer(callbacks.input_port);
            if !buffer.is_null() {
                let result = sys::pw_filter_queue_buffer(callbacks.input_port, buffer);
                if result < 0 {
                    callbacks
                        .failure
                        .publish(DriverProcessFailure::DependencyQueue(result));
                    return;
                }
            }
        }
    }
    if !callbacks.published.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: PipeWire supplies null or an IO position live for this invocation.
    let Some(position) = (unsafe { position.as_ref() }) else {
        callbacks
            .failure
            .publish(DriverProcessFailure::MissingPosition);
        return;
    };
    let Some(cycle) = DriverCycle::from_position(position) else {
        callbacks
            .failure
            .publish(DriverProcessFailure::InvalidPosition);
        return;
    };
    // SAFETY: publication transfers the prepared processor exclusively to RT;
    // control never accesses it again until native teardown has drained callbacks.
    let process = unsafe { &mut *callbacks.process.get() };
    process.process(&cycle);
    if let Some(failure) = process.failure() {
        callbacks.failure.publish(failure);
    }
}
