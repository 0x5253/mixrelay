//! Opt-in Driver capture `IOProc` and process-tap observations in a sole graph owner.
use mixrelay_audio_io::{ChannelLayout, StreamSpec, VirtualEndpointId, VirtualEndpointSpec};
use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_control::{DriverCommand, DriverState};
use mixrelay_macos_engine::endpoint_service::start;
use mixrelay_macos_transport::{
    app_lifecycle::{CONTROLLER_LEASE, ENGINE_LEASE, ProcessLease},
    device_io::{
        DeviceIo, mr_device_io_bridge_read, mr_device_io_connect, mr_device_io_create,
        mr_device_io_destroy,
    },
    endpoint_identity::{endpoint_uid, format_endpoint_id},
};
type EndpointId = [u8; 16];
use mixrelay_process_tap::{
    TapConfig, TapError, TapSession, TapStats, active_capture_owners, mr_process_tap_create,
    mr_process_tap_destroy, mr_process_tap_read, mr_process_tap_refresh_format,
    mr_process_tap_start, mr_process_tap_stats, mr_process_tap_stop,
};
use std::{
    ffi::CString,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdout, Command, Stdio},
    ptr::{self, NonNull},
    sync::mpsc::{Receiver, channel},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const A: EndpointId = [0xa1; 16];
const B: EndpointId = [0xb2; 16];
const RATE: u32 = 48_000;

fn until(message: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(5));
    }
}

struct InventoryOwner {
    _app: ProcessLease,
    _engine: ProcessLease,
    started: bool,
}

impl InventoryOwner {
    fn start() -> Self {
        assert_eq!(
            std::env::var("MIXRELAY_APPLICATION_MUTE_FIXTURE").as_deref(),
            Ok("1"),
            "explicit sole-launchd-owner fixture opt-in required"
        );
        let mut owner = Self {
            _app: ProcessLease::acquire(CONTROLLER_LEASE).expect("controller must not be running"),
            _engine: ProcessLease::acquire(ENGINE_LEASE).expect("engine must not be running"),
            started: false,
        };
        assert!(
            !mixrelay_macos_engine::native_lab::service_initialized(),
            "run only this native lab test"
        );
        let driver = DriverClient::new().unwrap().snapshot_blocking().unwrap();
        assert_eq!(
            driver.state,
            DriverState::Ready,
            "independent Driver must be installed"
        );
        assert!([A, B].iter().all(|id| {
            driver
                .settings
                .devices
                .iter()
                .all(|device| device.spec.id.as_str() != format_endpoint_id(*id))
        }));
        owner.started = true;
        assert_eq!(
            start(
                std::sync::Arc::new(mixrelay_time::StdClock::new()),
                mixrelay_realtime::Timebase::current()
            ),
            0,
            "graph source Mach service required"
        );
        owner
    }

    fn provision() {
        for id in [A, B] {
            let client = DriverClient::new().unwrap();
            let snapshot = client.snapshot_blocking().unwrap();
            client
                .mutate_blocking(&DriverCommand::Create {
                    expected_revision: snapshot.revision,
                    spec: VirtualEndpointSpec {
                        id: VirtualEndpointId::from(format_endpoint_id(id).as_str()),
                        display_name: format!("MixRelay owned mute fixture {}", endpoint_uid(id)),
                        default_sample_rate: RATE,
                        supported_sample_rates: vec![RATE],
                        render: Some(StreamSpec {
                            channels: 2,
                            channel_layout: ChannelLayout::Stereo,
                        }),
                        capture: Some(StreamSpec {
                            channels: 2,
                            channel_layout: ChannelLayout::Stereo,
                        }),
                    },
                })
                .unwrap();
        }
    }

    fn remove() {
        for id in [A, B] {
            let client = DriverClient::new().unwrap();
            let snapshot = client.snapshot_blocking().unwrap();
            if snapshot
                .settings
                .devices
                .iter()
                .any(|device| device.spec.id.as_str() == format_endpoint_id(id))
            {
                client
                    .mutate_blocking(&DriverCommand::Remove {
                        expected_revision: snapshot.revision,
                        endpoint_id: VirtualEndpointId::from(format_endpoint_id(id).as_str()),
                    })
                    .unwrap();
            }
        }
    }
}

impl Drop for InventoryOwner {
    fn drop(&mut self) {
        if self.started {
            Self::remove();
            mixrelay_macos_engine::endpoint_service::stop_lab_listener();
        }
    }
}

struct DriverCapture(NonNull<DeviceIo>);
impl DriverCapture {
    fn open(id: EndpointId) -> Self {
        let uid = CString::new(endpoint_uid(id)).unwrap();
        // SAFETY: the terminated UID is borrowed only during creation. This test
        // owns the returned IOProc until destruction drains all native callbacks.
        let pointer = unsafe { mr_device_io_create(uid.as_ptr(), true, 2, RATE) };
        let owner = Self(NonNull::new(pointer).expect("native Driver capture allocation"));
        // SAFETY: sole control owner; no other thread connects or reads this client.
        assert!(
            unsafe { mr_device_io_connect(owner.0.as_ptr()) },
            "Driver capture IOProc"
        );
        owner
    }

    fn read(&self, pcm: &mut [f32; 4096]) -> usize {
        // SAFETY: this sole reader owns 2048 complete stereo frames, while the
        // serialized CoreAudio callback owns the opposite bounded queue cursor.
        unsafe { mr_device_io_bridge_read(self.0.as_ptr(), pcm.as_mut_ptr(), 2048) as usize }
    }
}
impl Drop for DriverCapture {
    fn drop(&mut self) {
        // SAFETY: no reader escapes this owner. Failed native destruction retains
        // storage while CoreAudio drains; never release a callback-owned pointer.
        while !unsafe { mr_device_io_destroy(self.0.as_ptr()) } {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

struct ToneOwner {
    child: Child,
    replies: Receiver<String>,
    reader: Option<JoinHandle<()>>,
}

fn read_replies(stdout: ChildStdout, sender: &std::sync::mpsc::Sender<String>) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if sender.send(line).is_err() {
            break;
        }
    }
}

impl ToneOwner {
    fn start() -> Self {
        let executable = std::env::var("MIXRELAY_VIRTUAL_TONE_FIXTURE")
            .expect("existing MRVirtualToneSource fixture executable required");
        let child = Command::new(executable)
            .args(["--mixrelay-virtual-only", &endpoint_uid(A)])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("owned tone child");
        let (sender, replies) = channel();
        let mut owner = Self {
            child,
            replies,
            reader: None,
        };
        let stdout = owner.child.stdout.take().unwrap();
        owner.reader = Some(thread::spawn(move || read_replies(stdout, &sender)));
        owner.acknowledge(A);
        owner
    }

    fn acknowledge(&self, id: EndpointId) {
        assert_eq!(
            self.replies.recv_timeout(Duration::from_secs(5)).unwrap(),
            format!("ready {}", endpoint_uid(id))
        );
    }

    fn route(&mut self, id: EndpointId) {
        writeln!(
            self.child.stdin.as_mut().unwrap(),
            "route {}",
            endpoint_uid(id)
        )
        .unwrap();
        self.acknowledge(id);
    }
}

impl Drop for ToneOwner {
    fn drop(&mut self) {
        if let Some(mut input) = self.child.stdin.take() {
            let _ = writeln!(input, "quit");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.child.try_wait().is_ok_and(|status| status.is_some()) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct TapOwner(NonNull<TapSession>);

impl TapOwner {
    fn prepare(id: Option<EndpointId>) -> Self {
        let uid = id.map(|id| CString::new(endpoint_uid(id)).unwrap());
        let config = TapConfig {
            bundle_id: c"io.mixrelay.test-virtual-clock-source".as_ptr(),
            mute_while_captured: id.is_some(),
            ring_capacity_frames: 16_384,
            target_device_uid: uid.as_ref().map_or(ptr::null(), |uid| uid.as_ptr()),
            system_audio: false,
            excluded_bundle_ids: ptr::null(),
            excluded_bundle_count: 0,
        };
        let mut error = TapError::default();
        // SAFETY: configuration and terminated strings remain live during creation;
        // the returned unique native owner is retained through every serialized call.
        let raw = unsafe { mr_process_tap_create(&raw const config, &raw mut error) };
        Self(NonNull::new(raw).unwrap_or_else(|| panic!("tap create: {error}")))
    }

    fn start(&self) {
        // SAFETY: this test thread is the sole control/PCM consumer of this owner.
        assert_eq!(unsafe { mr_process_tap_start(self.0.as_ptr()) }.code, 0);
    }

    fn refresh(&self) -> TapError {
        // SAFETY: the retained owner has no concurrent control or PCM reader.
        unsafe { mr_process_tap_refresh_format(self.0.as_ptr()) }
    }

    fn stats(&self) -> TapStats {
        // SAFETY: native owner survives this atomic observation.
        unsafe { mr_process_tap_stats(self.0.as_ptr()) }
    }

    fn read(&self, output: &mut [f32; 4096]) -> usize {
        // SAFETY: sole reader; immutable stereo geometry fits 2048 complete frames.
        unsafe { mr_process_tap_read(self.0.as_ptr(), output.as_mut_ptr(), 2048) }
    }
}

impl Drop for TapOwner {
    fn drop(&mut self) {
        // SAFETY: no test reader escapes this owner; stop precedes async native drain.
        unsafe {
            if self.stats().running {
                let _ = mr_process_tap_stop(self.0.as_ptr());
            }
            mr_process_tap_destroy(self.0.as_ptr());
        }
    }
}

#[derive(Default)]
struct Pcm {
    samples: u32,
    energy: f64,
    peak: f32,
}

impl Pcm {
    fn add(&mut self, samples: &[f32]) {
        for &sample in samples {
            assert!(sample.is_finite());
            self.samples += 1;
            self.energy += f64::from(sample).powi(2);
            self.peak = self.peak.max(sample.abs());
        }
    }

    fn rms(&self) -> f64 {
        assert!(
            self.samples > RATE * 2,
            "missing actual PCM, not measured silence"
        );
        (self.energy / f64::from(self.samples)).sqrt()
    }
}

fn measure(capture: &TapOwner, id: EndpointId, quiet: bool, phase: &str) -> f64 {
    let start = Instant::now();
    let mut canonical = Pcm::default();
    let mut original = Pcm::default();
    let mut captured = [0.0; 4096];
    let mut rendered = [0.0; 4096];
    let driver_capture = DriverCapture::open(id);
    while start.elapsed() < Duration::from_secs(3) {
        assert_eq!(capture.refresh().code, 0, "Preserve capture must retrack");
        let count = capture.read(&mut captured);
        if start.elapsed() >= Duration::from_secs(1) {
            canonical.add(&captured[..count * 2]);
        }
        let frames = driver_capture.read(&mut rendered);
        if start.elapsed() >= Duration::from_secs(1) {
            original.add(&rendered[..frames * 2]);
        }
        thread::sleep(Duration::from_millis(5));
    }
    let capture_rms = canonical.rms();
    let original_rms = original.rms();
    println!(
        "LIVE {phase}: canonical rms={capture_rms:.8} FROM_OS rms={original_rms:.8} peak={:.8}",
        original.peak
    );
    assert!(capture_rms > 0.01 && canonical.peak > 0.01);
    if quiet {
        assert!(original_rms < 0.000_001 && original.peak < 0.000_01);
    } else {
        assert!(original_rms > 0.01 && original.peak > 0.01);
    }
    capture_rms
}

#[test]
fn live_owned_application_mute_preserves_capture_and_quiets_from_os() {
    let _inventory = InventoryOwner::start();
    InventoryOwner::provision();
    let mut tone = ToneOwner::start();
    let capture = TapOwner::prepare(None);
    capture.start();
    let baseline = measure(&capture, A, false, "preserve");
    let scoped = TapOwner::prepare(Some(A));
    assert!(!scoped.stats().running);
    measure(&capture, A, false, "stopped-prepare");
    scoped.start();
    let muted = measure(&capture, A, true, "scoped-muted");
    assert!((muted - baseline).abs() <= baseline * 0.05);
    assert!(scoped.stats().running && scoped.stats().callback_count > 0);
    drop(scoped);
    until("scoped owner did not drain", || {
        active_capture_owners() == 1
    });
    measure(&capture, A, false, "last-scoped-reference-released");
    let rerouted = TapOwner::prepare(Some(A));
    rerouted.start();
    measure(&capture, A, true, "second-explicit-lease");
    tone.route(B);
    until("external reroute did not invalidate scoped lease", || {
        rerouted.refresh().code == 9
    });
    drop(rerouted);
    until("revoked scoped owner did not drain", || {
        active_capture_owners() == 1
    });
    measure(&capture, B, false, "reroute-preserve-retracks");
    tone.route(A);
    measure(&capture, A, false, "return-without-new-mute-manifest");
    drop(capture);
    drop(tone);
    until("canonical owner did not drain", || {
        active_capture_owners() == 0
    });
    InventoryOwner::remove();
}
