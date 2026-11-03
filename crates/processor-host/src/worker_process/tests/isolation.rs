//! Real child failure injection through the same supervisor, PCM and recovery boundaries.

use super::*;
use crate::{
    AudioLayout, ProcessorControlRegistry, ProcessorStateError, dry_delay,
    hosted::HostedExternalProcessor, native_operation::Stage, shared_transport, timing,
};
use mixrelay_audio_engine::{HostedAudioProcessor, HostedLatencySource};
use mixrelay_audio_graph::NodeId;
use mixrelay_test_support::FixtureDir;

mod fixture;

const TEST: &str =
    "worker_process::tests::isolation::native_child_failures_preserve_peer_audio_and_recovery";
const MODE: &str = "MIXRELAY_TEST_NATIVE_FAILURE";
const PATH: &str = "MIXRELAY_TEST_NATIVE_TRANSPORT";
const AUDIBLE: [f32; 8] = [0.25, -0.5, 0.0, 0.75, -0.25, 0.5, 0.0, -0.75];

struct Instance {
    processor: HostedExternalProcessor,
    control: Arc<HostedProcessorControl>,
    pid: u32,
    directory: FixtureDir,
}

impl Instance {
    fn start(mode: &str) -> Result<Self, String> {
        let directory = FixtureDir::new("native-isolation").unwrap();
        let path = directory.join("audio");
        let layout = AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        };
        let (audio, mailbox) = shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        let control = Arc::new(HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::StdClock::new()),
        ));
        control.own_transport_file(path.clone());
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(MODE, mode)
            .env(PATH, &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let timeouts = Timeouts {
            startup: Duration::from_secs(2),
            native_call: Duration::from_millis(500),
            shutdown: Duration::from_millis(300),
        };
        let worker = match WorkerProcess::start(child, Arc::clone(&control), &audio, timeouts) {
            Ok(worker) => worker,
            Err(error) => {
                assert_reaped(pid);
                return Err(error);
            }
        };
        Ok(Self {
            processor: HostedExternalProcessor {
                worker: Some(worker),
                shared: Some(audio),
                config_path: directory.join("config"),
                layout,
                dry_delay: dry_delay::DryDelay::new(2, 2, 4).unwrap(),
                next_sequence: 1,
                timing: timing::TimingProbe::default(),
                control: Some(Arc::clone(&control)),
                registration: None,
            },
            control,
            pid,
            directory,
        })
    }

    fn callback(&mut self, input: &[f32; 8], expected: &[f32; 8]) {
        let mut output = [f32::NAN; 8];
        let measured = allocation_counter::measure(|| {
            assert!(self.processor.process(input, &mut output, 4));
            assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
        });
        assert_eq!(measured.count_total, 0);
    }

    fn wait_for_publication(&self, sequence: u64) {
        wait_until(|| {
            std::fs::read_to_string(self.directory.join("published"))
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .is_some_and(|value| value >= sequence)
        });
    }

    fn finish(mut self) {
        self.control
            .shared
            .header()
            .shutdown
            .store(1, Ordering::Release);
        drop(self.processor.worker.take());
        assert_reaped(self.pid);
    }
}

fn assert_reaped(pid: u32) {
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: signal zero only queries the exact fixture PID returned by our owned Child.
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "fixture child remains alive"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

/// Only a stalled child reaches the guard; loaded hosted runners can take seconds per transition.
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "isolated child did not reach its expected state"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn native_child_failures_preserve_peer_audio_and_recovery() {
    if let Ok(mode) = std::env::var(MODE) {
        fixture::run(&mode);
        return;
    }
    let mut peer = Instance::start("healthy").unwrap();
    peer.callback(&AUDIBLE, &[0.0; 8]);
    peer.wait_for_publication(1);
    let registry = ProcessorControlRegistry::default();
    let peer_node = NodeId::from("healthy-peer");
    let _peer_registration = registry.register(&peer_node, &peer.control);
    for mode in ["early_exit", "load_hang"] {
        let error = Instance::start(mode)
            .err()
            .expect("startup failure must be rejected");
        assert!(error.contains("load"), "{mode}: {error}");
        peer.callback(&AUDIBLE, &AUDIBLE.map(|sample| sample * 2.0));
        peer.wait_for_publication(peer.processor.next_sequence - 1);
        eprintln!("native fixture {mode}: rejected during load, child reaped, peer PCM preserved");
    }
    for mode in [
        "crash",
        "panic_abort",
        "process_hang",
        "state_hang",
        "editor_hang",
    ] {
        verify_failure(mode, &mut peer, &registry, &peer_node);
    }
    verify_late_reply(&mut peer);
    assert_eq!(registry.failed_worker_instance(&peer_node), None);
    peer.finish();
}

fn verify_failure(
    mode: &str,
    peer: &mut Instance,
    registry: &ProcessorControlRegistry,
    peer_node: &NodeId,
) {
    let mut broken = Instance::start(mode).unwrap();
    let node = NodeId::from(mode);
    let _registration = registry.register(&node, &broken.control);
    if mode == "state_hang" {
        assert_eq!(
            broken.control.snapshot_state(Duration::ZERO),
            Err(ProcessorStateError::Timeout)
        );
        wait_until(|| {
            broken
                .control
                .shared
                .header()
                .native_operation
                .snapshot()
                .active_stage()
                == Some(Stage::State)
        });
    } else if mode == "editor_hang" {
        broken.control.set_editor_open(true);
        wait_until(|| {
            broken
                .control
                .shared
                .header()
                .native_operation
                .snapshot()
                .active_stage()
                == Some(Stage::Editor)
        });
    }
    broken.callback(&AUDIBLE, &[0.0; 8]);
    wait_until(|| {
        broken
            .control
            .shared
            .header()
            .worker_status
            .load(Ordering::Acquire)
            == WORKER_FAILED
    });
    broken.callback(&[0.0; 8], &AUDIBLE);
    for _ in 0..8 {
        broken.callback(&[0.0; 8], &[0.0; 8]);
        peer.callback(&AUDIBLE, &AUDIBLE.map(|sample| sample * 2.0));
        peer.wait_for_publication(peer.processor.next_sequence - 1);
    }
    let id = broken.control.shared.header().instance_id;
    assert_eq!(registry.failed_worker_instance(&node), Some(id));
    assert_eq!(registry.failed_worker_instance(peer_node), None);
    assert_eq!(broken.control.frames(), None);
    assert_eq!(
        broken.control.snapshot_state(Duration::ZERO),
        Err(ProcessorStateError::Unavailable)
    );
    registry
        .begin_failed_worker_retry(&node, id, Duration::ZERO)
        .unwrap();
    eprintln!(
        "native fixture {mode}: failed instance {id}, aligned dry then quiet, peer PCM preserved, retry available"
    );
    broken.finish();
}

fn verify_late_reply(peer: &mut Instance) {
    let mut late = Instance::start("late").unwrap();
    late.callback(&AUDIBLE, &[0.0; 8]);
    wait_until(|| {
        late.control
            .shared
            .header()
            .dsp_operation
            .snapshot()
            .active_stage()
            == Some(Stage::Process)
    });
    late.callback(&[0.0; 8], &AUDIBLE);
    std::fs::write(late.directory.join("release"), []).unwrap();
    late.wait_for_publication(1);
    late.callback(&[0.0; 8], &[0.0; 8]);
    // The next callback must acknowledge packet 2 before an admitted packet 3 can publish.
    late.wait_for_publication(2);
    late.callback(&[0.0; 8], &[0.0; 8]);
    peer.callback(&AUDIBLE, &AUDIBLE.map(|sample| sample * 2.0));
    peer.wait_for_publication(peer.processor.next_sequence - 1);
    eprintln!("native fixture late: stale wet rejected, next quiet and peer PCM preserved");
    late.finish();
}
