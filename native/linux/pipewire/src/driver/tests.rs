use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize},
};

struct Counter {
    calls: Arc<AtomicUsize>,
    position: Arc<AtomicU64>,
    dropped: Arc<AtomicBool>,
    prepared: Arc<AtomicBool>,
}

impl DriverProcess for Counter {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        inputs.ensure_live()?;
        assert_eq!(
            self.calls.load(Ordering::Acquire),
            0,
            "DSP ran before control preparation"
        );
        self.prepared.store(true, Ordering::Release);
        Ok(())
    }
    fn process(&mut self, cycle: &DriverCycle) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.position.store(cycle.position, Ordering::Relaxed);
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

#[cfg(feature = "native-lab")]
struct RejectedInputs(Counter);

#[cfg(feature = "native-lab")]
impl DriverProcess for RejectedInputs {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        inputs.ensure_live()?;
        Err(DriverError::Input("fixture rejects required input".into()))
    }

    fn process(&mut self, cycle: &DriverCycle) {
        self.0.process(cycle);
    }
}

fn counter() -> Counter {
    Counter {
        calls: Arc::new(AtomicUsize::new(0)),
        position: Arc::new(AtomicU64::new(0)),
        dropped: Arc::new(AtomicBool::new(false)),
        prepared: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn expired_input_capability_cannot_revive_driver_or_create_a_binding() {
    let inputs = GraphInputs(Weak::new());
    assert!(matches!(inputs.ensure_live(), Err(DriverError::Input(_))));
    assert!(matches!(
        inputs.bind(17, "monitor_FL"),
        Err(DriverError::Input(_))
    ));
    drop(GraphInputBinding {
        owner: inputs.0,
        key: 1,
    });
}

#[test]
fn owner_format_validates_and_publishes_exact_scheduling_preferences() {
    for (rate, frames) in [
        (8_000, 1),
        (44_100, 256),
        (48_000, 128),
        (96_000, 32),
        (384_000, 8192),
    ] {
        let properties = DriverFormat::new(rate, frames).unwrap().properties();
        assert_eq!(
            properties.get("node.rate"),
            Some(format!("1/{rate}").as_str())
        );
        assert_eq!(
            properties.get("node.latency"),
            Some(format!("{frames}/{rate}").as_str())
        );
        assert!(properties.get("clock.force-rate").is_none());
        assert!(properties.get("node.force-rate").is_none());
        assert!(properties.get("node.force-quantum").is_none());
    }
    for (rate, frames) in [
        (0, 128),
        (7_999, 128),
        (384_001, 128),
        (48_000, 0),
        (48_000, 8193),
    ] {
        assert!(matches!(
            DriverFormat::new(rate, frames),
            Err(DriverError::InvalidFormat)
        ));
    }
}

fn position() -> spa::sys::spa_io_position {
    // SAFETY: generated SPA POD consists of scalar fields and arrays; zero is
    // its native empty initialization. The fixture owns this complete value.
    let mut value: spa::sys::spa_io_position = unsafe { std::mem::zeroed() };
    value.clock.rate.num = 1;
    value.clock.rate.denom = 48_000;
    value.clock.id = 23;
    value.clock.duration = 128;
    value.clock.nsec = 99;
    value.clock.position = u64::MAX - 2;
    value
}

#[test]
fn cycle_retains_native_clock_and_rejects_unrepresentable_durations() {
    let mut value = position();
    let cycle = DriverCycle::from_position(&value).unwrap();
    assert_eq!(
        cycle,
        DriverCycle {
            position: u64::MAX - 2,
            monotonic_ns: 99,
            clock_id: 23,
            rate: 48_000,
            frames: 128
        }
    );
    assert_eq!(size_of::<DriverCycle>(), 32);
    assert_eq!(align_of::<DriverCycle>(), 8);
    value.clock.rate.num = 2;
    assert!(DriverCycle::from_position(&value).is_none());
    value.clock.rate.num = 1;
    value.clock.duration = u64::from(u32::MAX) + 1;
    assert!(DriverCycle::from_position(&value).is_none());
}

#[test]
fn callback_owns_processor_and_main_state_observations_do_not_borrow_it() {
    let mut callbacks = Callbacks {
        state: AtomicI32::new(0),
        published: AtomicBool::new(false),
        failure: FailurePublication::default(),
        process: UnsafeCell::new(counter()),
        input_port: std::ptr::null_mut(),
    };
    let calls = Arc::clone(&callbacks.process.get_mut().calls);
    let data = (&raw const callbacks).cast_mut().cast();
    let mut native = position();
    // SAFETY: valid callback storage and position; native negotiation must not borrow DSP.
    unsafe { process::<Counter>(data, &raw mut native) };
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    callbacks.published.store(true, Ordering::Release);
    let allocation = allocation_counter::measure(|| {
        for _ in 0..10_000 {
            // SAFETY: complete fixture, serialized callback, disjoint valid position.
            unsafe { process::<Counter>(data, &raw mut native) };
        }
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(calls.load(Ordering::Relaxed), 10_000);
    // SAFETY: retained fixture; null position is the callback's explicit rejection path.
    unsafe { process::<Counter>(data, std::ptr::null_mut()) };
    native.clock.rate.num = 0;
    // SAFETY: complete but invalid native clock; must not invoke the processor.
    unsafe { process::<Counter>(data, &raw mut native) };
    assert_eq!(calls.load(Ordering::Relaxed), 10_000);
    assert_eq!(
        callbacks.failure.read(),
        Some(DriverProcessFailure::MissingPosition)
    );
    // SAFETY: main callback touches only its atomic state, not processor storage.
    callbacks.input_port = std::ptr::dangling_mut::<u8>().cast();
    unsafe { state_changed::<Counter>(data, 0, 3, std::ptr::null()) };
    assert_eq!(callbacks.state.load(Ordering::Acquire), 3);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_graph_driver_drains_before_processor_drop_and_releases_resources() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    assert!(std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE").is_some());
    let resources = || {
        (
            std::fs::read_dir("/proc/self/fd").unwrap().count(),
            std::fs::read_dir("/proc/self/task").unwrap().count(),
        )
    };
    for _ in 0..2 {
        drop(GraphDriver::open(counter(), DriverFormat::new(48_000, 128).unwrap()).unwrap());
    }
    let before = resources();
    let rejected = counter();
    let rejected_calls = Arc::clone(&rejected.calls);
    let rejected_drop = Arc::clone(&rejected.dropped);
    assert!(matches!(
        GraphDriver::open(
            RejectedInputs(rejected),
            DriverFormat::new(48_000, 128).unwrap()
        ),
        Err(DriverError::Input(_))
    ));
    assert_eq!(
        rejected_calls.load(Ordering::Acquire),
        0,
        "failed setup activated DSP"
    );
    assert!(rejected_drop.load(Ordering::Acquire));
    for _ in 0..16 {
        let process = counter();
        let calls = Arc::clone(&process.calls);
        let dropped = Arc::clone(&process.dropped);
        let prepared = Arc::clone(&process.prepared);
        let driver = GraphDriver::open(process, DriverFormat::new(48_000, 128).unwrap()).unwrap();
        assert!(prepared.load(Ordering::Acquire));
        let started = Instant::now();
        while !driver.ready() || calls.load(Ordering::Acquire) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "native driver never ran"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!dropped.load(Ordering::Acquire));
        drop(driver);
        assert!(dropped.load(Ordering::Acquire));
        let last = calls.load(Ordering::Acquire);
        thread::sleep(Duration::from_millis(5));
        assert_eq!(
            calls.load(Ordering::Acquire),
            last,
            "callback survived its owner"
        );
    }
    assert_eq!(
        resources(),
        before,
        "FD/thread leak after native filter retirement"
    );
}

#[test]
fn callback_geometry_rejects_zero_or_unbounded_native_work() {
    let mut native = position();
    native.clock.duration = 32_768;
    assert_eq!(DriverCycle::from_position(&native).unwrap().frames, 32_768);
    for duration in [0, 32_769, u64::MAX] {
        native.clock.duration = duration;
        assert_eq!(DriverCycle::from_position(&native), None);
    }
    native.clock.duration = 128;
    for rate in [0, 7_999, 384_001] {
        native.clock.rate.denom = rate;
        assert_eq!(DriverCycle::from_position(&native), None);
    }
}

struct FaultedProcessor {
    calls: usize,
    failure: Option<DriverProcessFailure>,
}

impl DriverProcess for FaultedProcessor {
    fn process(&mut self, cycle: &DriverCycle) {
        self.calls += 1;
        self.failure = Some(DriverProcessFailure::FrameBudget {
            native_frames: cycle.frames,
            native_rate: cycle.rate,
            stream_rate: 192_000,
            capacity_frames: 32_768,
        });
    }
    fn failure(&self) -> Option<DriverProcessFailure> {
        self.failure
    }
}

#[test]
fn first_source_fault_is_control_readable_without_another_native_callback() {
    let mut callbacks = Callbacks {
        state: AtomicI32::new(0),
        published: AtomicBool::new(true),
        failure: FailurePublication::default(),
        process: UnsafeCell::new(FaultedProcessor {
            calls: 0,
            failure: None,
        }),
        input_port: std::ptr::null_mut(),
    };
    let data = (&raw const callbacks).cast_mut().cast();
    let mut native = position();
    let allocation = allocation_counter::measure(|| {
        // SAFETY: this fixture retains the complete callback and native position.
        unsafe { process::<FaultedProcessor>(data, &raw mut native) };
    });
    assert_eq!(allocation.count_total, 0);
    let expected = Some(DriverProcessFailure::FrameBudget {
        native_frames: 128,
        native_rate: 48_000,
        stream_rate: 192_000,
        capacity_frames: 32_768,
    });
    assert_eq!(callbacks.failure.read(), expected);
    callbacks
        .failure
        .publish(DriverProcessFailure::DependencyQueue(-22));
    assert_eq!(callbacks.failure.read(), expected);
    // SAFETY: the same pinned fixture is called again; the terminal gate must stop it.
    unsafe { process::<FaultedProcessor>(data, &raw mut native) };
    assert_eq!(callbacks.process.get_mut().calls, 1);
    assert_eq!(callbacks.failure.read(), expected);

    let mut peer = Callbacks {
        state: AtomicI32::new(0),
        published: AtomicBool::new(true),
        failure: FailurePublication::default(),
        process: UnsafeCell::new(counter()),
        input_port: std::ptr::null_mut(),
    };
    let peer_data = (&raw const peer).cast_mut().cast();
    // SAFETY: a separate complete callback owner borrows the same valid native value serially.
    unsafe { process::<Counter>(peer_data, &raw mut native) };
    assert_eq!(peer.process.get_mut().calls.load(Ordering::Relaxed), 1);
    assert_eq!(peer.failure.read(), None);
}
