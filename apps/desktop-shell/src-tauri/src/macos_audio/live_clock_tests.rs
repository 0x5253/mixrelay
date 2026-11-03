//! Opt-in real process tap + actual desktop SRC pump + isolated v2 registry.
//! Never attaches to the installed engine's graph or a physical/default output.
use super::super::*;
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    time::Instant,
};

#[repr(C)]
struct Registry {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Default)]
struct Clock {
    epoch: u64,
    native_frames: u64,
    native_rate: u32,
    graph_rate: u32,
    running: u32,
    synchronized: u32,
}
unsafe extern "C" {
    fn mr_application_clock_fixture_create(
        transport: *mut *mut NativeApplicationSourceTransport,
    ) -> *mut Registry;
    fn mr_application_clock_fixture_read(
        registry: *mut Registry,
        samples: *mut f32,
        rate: u32,
    ) -> u32;
    fn mr_application_clock_fixture_clock(registry: *mut Registry, clock: *mut Clock) -> bool;
    fn mr_engine_source_registry_destroy(registry: *mut Registry);
}

struct RegistryOwner(NonNull<Registry>);
impl RegistryOwner {
    fn create() -> (Self, ApplicationSourceTransport) {
        let mut transport = std::ptr::null_mut();
        // SAFETY: test helper returns independently owned registry and producer mappings.
        let pointer = unsafe { mr_application_clock_fixture_create(&raw mut transport) };
        let registry = Self(NonNull::new(pointer).expect("isolated source registry"));
        (
            registry,
            ApplicationSourceTransport {
                native: NonNull::new(transport).expect("v2 producer"),
            },
        )
    }
    fn read(&self, output: &mut [f32; 256], rate: u32) -> u32 {
        // SAFETY: this test is the registry's single consumer; buffer is128 stereo frames.
        unsafe { mr_application_clock_fixture_read(self.0.as_ptr(), output.as_mut_ptr(), rate) }
    }
    fn clock(&self) -> Clock {
        let mut clock = Clock::default();
        // SAFETY: valid owned registry, writable ABI-sized snapshot.
        assert!(unsafe { mr_application_clock_fixture_clock(self.0.as_ptr(), &raw mut clock) });
        clock
    }
    fn capture_time(&self, rate: u32) -> Option<u64> {
        // SAFETY: the C fixture forwards the current Rust SourceRegistry allocation
        // unchanged. This owner retains it; this thread owns its
        // sole rate/consumer authority while the pump owns only the producer.
        let registry = unsafe {
            &*self
                .0
                .as_ptr()
                .cast::<mixrelay_macos_transport::source_registry::SourceRegistry>()
        };
        // SAFETY: the literal is a complete retained NUL-terminated source ID.
        let token = unsafe {
            mixrelay_macos_transport::source_registry::mr_engine_source_id_token(
                c"owned-live-clock-fixture".as_ptr(),
            )
        };
        registry.source_time(token, rate, 128)
    }
}
impl Drop for RegistryOwner {
    fn drop(&mut self) {
        // SAFETY: the pump joins before this independently mapped registry is destroyed.
        unsafe { mr_engine_source_registry_destroy(self.0.as_ptr()) };
    }
}

struct ToneOwner(Child);
impl ToneOwner {
    fn start() -> Self {
        let executable = std::env::var("MIXRELAY_VIRTUAL_TONE_FIXTURE")
            .expect("explicit owned tone fixture path");
        let endpoint_id = std::env::var("MIXRELAY_VIRTUAL_TONE_ENDPOINT_ID")
            .expect("explicit owned canonical endpoint UUID");
        let endpoint = parse_endpoint_id(&endpoint_id).expect("canonical nonzero endpoint UUID");
        let uid = endpoint_uid(endpoint);
        let mut owned = Self(
            Command::new(executable)
                .arg("--mixrelay-virtual-only")
                .arg(&uid)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("owned virtual-only tone"),
        );
        let stdout = owned.0.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
            let _ = sender.send(result);
        });
        let ready = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("bounded tone readiness")
            .unwrap();
        reader.join().unwrap();
        assert_eq!(ready.trim(), format!("ready {uid}"));
        owned
    }
}
impl Drop for ToneOwner {
    fn drop(&mut self) {
        if let Some(mut input) = self.0.stdin.take() {
            let _ = writeln!(input, "quit");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.0.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Only the exact child this fixture spawned, never an app-name kill.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn measure_phase(registry: &RegistryOwner, rate: u32, capture: &CaptureProbeSession) -> Clock {
    // The three prepared FIFOs together retain less than two seconds at every
    // tested rate. A larger gap identifies a stale epoch or a different host origin.
    const MAX_RETAINED_NANOS: u64 = 2_000_000_000;
    let mut pcm = [0.0_f32; 256];
    let quantum = Duration::from_secs_f64(128.0 / f64::from(rate));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut next = Instant::now();
    let mut frames = 0_u32;
    let mut crossings = 0_u32;
    let mut previous = 0.0_f32;
    let mut peak = 0.0_f32;
    let mut energy = 0.0_f64;
    while Instant::now() < deadline {
        let valid = registry.read(&mut pcm, rate);
        for frame in pcm[..usize::try_from(valid).unwrap() * 2].chunks_exact(2) {
            let sample = frame[0];
            assert!(sample.is_finite());
            if previous <= 0.0 && sample > 0.0 {
                crossings += 1;
            }
            previous = sample;
            peak = peak.max(sample.abs());
            energy += f64::from(sample).powi(2);
        }
        frames += valid;
        next += quantum;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let rms = (energy / f64::from(frames.max(1))).sqrt();
    let frequency = f64::from(crossings) * f64::from(rate) / f64::from(frames.max(1));
    let clock = registry.clock();
    println!(
        "LIVE application native={} graph={} epoch={} valid_frames={frames} native_frames={} peak={peak:.6} rms={rms:.6} frequency={frequency:.3} synchronized={}",
        clock.native_rate, rate, clock.epoch, clock.native_frames, clock.synchronized
    );
    let health = capture.stats();
    println!("LIVE tap health: {health:?}");
    let first = registry
        .capture_time(rate)
        .expect("real SRC PCM retains acquisition coordinates");
    let native = mixrelay_realtime::Timebase::current().nanoseconds(health.host_time);
    assert!(
        native.abs_diff(first) < MAX_RETAINED_NANOS,
        "SRC coordinates lost the native host origin: {first} vs {native}"
    );
    assert!(frames > rate, "less than one second of real captured PCM");
    assert!(peak > 0.01 && rms > 0.01, "owned440Hz tone absent");
    assert!(
        (420.0..460.0).contains(&frequency),
        "sample-rate mismatch changed pitch"
    );
    assert_eq!(clock.graph_rate, rate);
    assert_eq!(clock.running, 1);
    clock
}

#[test]
fn live_owned_application_tap_v2_src_rate_changes() {
    let _tone = ToneOwner::start();
    let (registry, transport) = RegistryOwner::create();
    let capture = Arc::new(
        CaptureProbeSession::start("io.mixrelay.test-virtual-clock-source", None)
            .expect("owned process tap; permission denial requires user action, never reset"),
    );
    assert_eq!(capture.stats().channels, 2);
    let worker_capture = capture.acquire_reader().expect("sole fixture PCM reader");
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let failure = Arc::new(Mutex::new(None));
    let worker_failure = failure.clone();
    let handoff = Arc::new(capture_transaction::CaptureHandoff::default());
    let worker_handoff = Arc::clone(&handoff);
    let playback = Arc::new(capture_transaction::CaptureHandoff::default());
    let worker_playback = Arc::clone(&playback);
    let worker = std::thread::spawn(move || {
        if let Err(error) = clock_pump::run_with_handoff(
            worker_capture,
            transport,
            2,
            PumpControl {
                stop: &worker_stop,
                capture: &worker_handoff,
                playback: &worker_playback,
            },
        ) {
            *worker_failure.lock().unwrap() = Some(error);
        }
    });
    let pump = AudioPump {
        stop,
        worker: Some(worker),
        handoff,
        playback,
    };
    let first = measure_phase(&registry, 44_100, &capture);
    let second = measure_phase(&registry, 48_000, &capture);
    let third = measure_phase(&registry, 44_100, &capture);
    assert_ne!(first.epoch, second.epoch);
    assert_ne!(second.epoch, third.epoch);
    assert!(
        third.native_frames > second.native_frames && second.native_frames > first.native_frames
    );
    let deadline = Instant::now() + Duration::from_millis(100);
    let health = loop {
        let health = capture.stats();
        if health.clock_valid {
            break health;
        }
        assert!(
            Instant::now() < deadline,
            "real tap did not publish a valid buffer timestamp"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    println!(
        "LIVE tap buffer timestamp: sample={} host={} discontinuities={}",
        health.sample_time, health.host_time, health.clock_discontinuities
    );
    assert!(health.sample_time.is_finite() && health.host_time != 0);
    drop(pump);
    let failure = failure.lock().unwrap().clone();
    assert!(failure.is_none(), "pump failed: {failure:?}");
    println!("LIVE application component qualification passed; installed engine graph untouched");
}
