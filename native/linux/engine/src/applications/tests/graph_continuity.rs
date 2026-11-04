use super::*;
use mixrelay_audio_graph::composition::{CompositionError, SharedSource};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, ApplicationSelector, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    CompileContext, CompileLimits, DeviceSelector, EdgeSpec, FROM_OS_PORT_ID, GraphCompiler,
    GraphDocument, NodeConfig, NodeId, NodeSpec, RenderFormat, TO_OS_PORT_ID,
    VirtualEndpointConfig, VirtualEndpointReference,
};
use mixrelay_audio_io::{
    AudioDeviceDescriptor, ChannelLayout, SourcePlaybackPolicy, StreamSpec, VirtualEndpointId,
    VirtualEndpointSpec,
};
#[cfg(feature = "native-lab")]
use mixrelay_engine_core::GraphResourceBackend;
use mixrelay_engine_core::catalog_execution::{
    ApplicationIdentityResolver, CatalogExecutionContext, compose_catalog,
};
#[cfg(feature = "native-lab")]
use mixrelay_test_support::{frame_sequence::SequenceContinuity, tone_continuity::ToneContinuity};
use serde_json::Value;
#[cfg(feature = "native-lab")]
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "native-lab")]
mod driver_fixture;

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: u32 = 128;
#[cfg(feature = "native-lab")]
const WINDOW_FRAMES: usize = 480_000;
#[cfg(feature = "native-lab")]
const READ_FRAMES: usize = 128;
#[cfg(feature = "native-lab")]
const CAPTURE_NODE: &str = "graph-continuity";
#[cfg(feature = "native-lab")]
const SOURCE_APP_ID: &str = "org.mixrelay.fixture.application";
#[cfg(feature = "native-lab")]
const SOURCE_NODE_NAME: &str = "io.mixrelay.lab-application";
#[cfg(feature = "native-lab")]
const SINK_APP_ID: &str = "org.mixrelay.fixture.mute-sink";
const SINK_NODE_NAME: &str = "io.mixrelay.fixture.mute-sink";
const ENGINE_OUTPUT_NODE_NAME: &str = "io.mixrelay.engine-output.device-output";
const FORWARD_ENDPOINT_ID: &str = "lab-source-mute-X";
#[cfg(feature = "native-lab")]
const UNRELATED_APP_ID: &str = "org.mixrelay.fixture.unrelated-880";
#[cfg(feature = "native-lab")]
const COLLECTION_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(feature = "native-lab")]
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);

#[cfg(feature = "native-lab")]
fn graph_clock_rate() -> u32 {
    match std::env::var("MIXRELAY_LAB_GRAPH_RATE").as_deref() {
        Ok("44100") => 44_100,
        Ok("48000") | Err(std::env::VarError::NotPresent) => SAMPLE_RATE,
        value => panic!("unsupported native graph rate: {value:?}"),
    }
}

#[cfg(feature = "native-lab")]
struct GraphCaptureEvidence {
    directory: PathBuf,
    frames: Vec<[f32; 2]>,
    transfer_before: Option<mixrelay_linux_pipewire::TransferSnapshot>,
    transfer_after: Option<mixrelay_linux_pipewire::TransferSnapshot>,
    source_sample_faults: u64,
    saved: bool,
}

#[cfg(feature = "native-lab")]
impl GraphCaptureEvidence {
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            frames: Vec::with_capacity(WINDOW_FRAMES),
            transfer_before: None,
            transfer_after: None,
            source_sample_faults: 0,
            saved: false,
        }
    }

    fn collect(&mut self, captures: &mut CaptureSet) {
        self.transfer_before = Some(captures.transfer_snapshot());
        let deadline = Instant::now() + COLLECTION_TIMEOUT;
        let mut samples = [0.0_f32; READ_FRAMES * 2];
        while self.frames.len() < WINDOW_FRAMES {
            let remaining = WINDOW_FRAMES - self.frames.len();
            let sample_limit = remaining.min(READ_FRAMES) * 2;
            let outcome = captures.read(CAPTURE_NODE, &mut samples[..sample_limit]);
            self.source_sample_faults = self
                .source_sample_faults
                .saturating_add(outcome.source_sample_faults);
            if outcome.discontinuity {
                self.transfer_after = Some(captures.transfer_snapshot());
            }
            assert_eq!(
                outcome.activity,
                mixrelay_audio_io::process::InputActivity::Active,
                "separate ProcessId CaptureSet evidence tap became inactive"
            );
            if outcome.frames == 0 {
                assert!(
                    Instant::now() < deadline,
                    "separate capture evidence tap stalled"
                );
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            assert!(
                !outcome.discontinuity,
                "separate capture evidence tap discontinuity"
            );
            let frames = usize::try_from(outcome.frames).expect("bounded frame count");
            assert!(
                frames <= sample_limit / 2,
                "capture evidence tap exceeded read bound"
            );
            self.frames.extend(
                samples[..frames * 2]
                    .chunks_exact(2)
                    .map(|frame| [frame[0], frame[1]]),
            );
            assert!(
                Instant::now() < deadline,
                "10-second capture evidence tap timed out"
            );
        }
        self.transfer_after = Some(captures.transfer_snapshot());
    }

    fn assert_no_native_capture_loss(&self) {
        assert_eq!(
            self.source_sample_faults, 0,
            "independent capture replaced non-finite source samples"
        );
        let before = self.transfer_before.expect("capture start counters");
        let after = self.transfer_after.expect("capture end counters");
        for snapshot in [before, after] {
            assert_eq!(snapshot.streams, 1, "exactly one evidence capture owner");
            assert_eq!(
                snapshot.counters.failures[0].rejected_calls, 0,
                "independent capture rejected native ingress: {snapshot:?}"
            );
        }
        assert_eq!(before.owner_sequence, after.owner_sequence);
        assert_eq!(
            after
                .counters
                .engine_read_frames
                .wrapping_sub(before.counters.engine_read_frames),
            WINDOW_FRAMES as u64,
            "capture evidence consumed exactly the retained PCM window"
        );
        assert_eq!(
            after.counters.consumer_discard_calls,
            before.counters.consumer_discard_calls
        );
        assert_eq!(
            after.counters.native_restart_calls,
            before.counters.native_restart_calls
        );
    }

    fn save(&mut self, evidence: &Value) -> std::io::Result<()> {
        self.saved = true;
        self.write_files(evidence)
    }

    fn write_files(&self, evidence: &Value) -> std::io::Result<()> {
        let process_id = std::process::id();
        let raw_name = format!("graph-capture-{process_id}.f32");
        let mut raw = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.directory.join(&raw_name))?,
        );
        for sample in self.frames.iter().flatten() {
            raw.write_all(&sample.to_ne_bytes())?;
        }
        raw.flush()?;
        let mut metadata = BufWriter::new(
            OpenOptions::new().write(true).create_new(true).open(
                self.directory
                    .join(format!("graph-continuity-{process_id}.json")),
            )?,
        );
        serde_json::to_writer(
            &mut metadata,
            &serde_json::json!({
                "sample_rate": SAMPLE_RATE,
                "channels": 2,
                "measured_frames": self.frames.len(),
                "raw_file": raw_name,
                "source_sample_faults": self.source_sample_faults,
                "captureTransferWindow": {
                    "before": self.transfer_before,
                    "after": self.transfer_after,
                },
                "evidence": evidence,
            }),
        )
        .map_err(std::io::Error::other)?;
        metadata.flush()
    }
}

#[cfg(feature = "native-lab")]
impl Drop for GraphCaptureEvidence {
    fn drop(&mut self) {
        if !self.saved {
            self.saved = true;
            if let Err(error) = self.write_files(&serde_json::json!({"complete": false})) {
                eprintln!("could not save bounded graph continuity PCM: {error}");
            }
        }
    }
}

struct DriverCounters {
    #[cfg(feature = "native-lab")]
    cycles: AtomicU64,
    frames: AtomicU64,
    clock: AtomicU64,
    position: AtomicU64,
    discontinuities: AtomicU64,
    clock_diagnostics: Arc<crate::processing::clock::DiscontinuityMailbox>,
    transfers: [[AtomicU64; mixrelay_linux_pipewire::TRANSFER_SNAPSHOT_WORDS]; 3],
    transfer_generation: AtomicU64,
}

impl Default for DriverCounters {
    fn default() -> Self {
        Self {
            #[cfg(feature = "native-lab")]
            cycles: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            clock: AtomicU64::new(0),
            position: AtomicU64::new(0),
            discontinuities: AtomicU64::new(0),
            clock_diagnostics: Arc::default(),
            transfers: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            transfer_generation: AtomicU64::new(0),
        }
    }
}

impl DriverCounters {
    fn transfer_observation(&self) -> Value {
        self.transfer_observation_with(|| {})
    }

    // This hook deterministically replaces an owner between the payload and
    // version reads in unit tests; the actual control reader passes a no-op.
    fn transfer_observation_with(&self, mut after_payload: impl FnMut()) -> Value {
        const MAX_ATTEMPTS: u32 = 3;
        let native_cycle = |cycle: &mixrelay_linux_pipewire::DriverCycle| {
            serde_json::json!({"clock_id": cycle.clock_id, "rate": cycle.rate,
                "position": cycle.position, "frames": cycle.frames,
                "monotonic_ns": cycle.monotonic_ns})
        };
        let latest_clock_discontinuity = self.clock_diagnostics.read_since(0).map(|read| {
            serde_json::json!({"previous": native_cycle(&read.event.previous),
                "current": native_cycle(&read.event.current),
                "mapped_epoch": read.event.mapped_epoch,
                "render_position": read.event.render_position,
                "event_number": read.event_number, "overwritten": read.overwritten})
        });
        // This latest-only read is separate from the owner snapshot. Counts are lifetime
        // totals, and null means no event or an overlapping writer; reads do not consume it.
        let mailbox_contract = "separate latest control read; counts lifetime events";
        for attempt in 1..=MAX_ATTEMPTS {
            let generation = self.transfer_generation.load(Ordering::SeqCst);
            if generation & 1 != 0 {
                continue;
            }
            let sets: [mixrelay_linux_pipewire::TransferSnapshot; 3] = std::array::from_fn(|set| {
                mixrelay_linux_pipewire::TransferSnapshot::from_words(&std::array::from_fn(
                    |word| self.transfers[set][word].load(Ordering::SeqCst),
                ))
            });
            let clock = self.clock.load(Ordering::SeqCst);
            let position = self.position.load(Ordering::SeqCst);
            let frames = self.frames.load(Ordering::SeqCst);
            let discontinuities = self.discontinuities.load(Ordering::SeqCst);
            after_payload();
            if self.transfer_generation.load(Ordering::SeqCst) == generation {
                return serde_json::json!({
                    "available": true, "generation": generation, "attempts": attempt,
                    "engineClock": {"id": clock >> 32,
                        "rate": u32::try_from(clock & u64::from(u32::MAX))
                            .expect("masked native clock rate fits u32"),
                        "position": position, "callbackFrames": frames,
                        "discontinuities": discontinuities},
                    "capture": sets[0], "endpoint": sets[1], "device": sets[2],
                    "latestClockDiscontinuity": latest_clock_discontinuity,
                    "clockDiscontinuityContract": mailbox_contract,
                });
            }
        }
        serde_json::json!({"available": false, "attempts": MAX_ATTEMPTS,
            "reason": "test owner publication changed or remained in progress",
            "latestClockDiscontinuity": latest_clock_discontinuity,
            "clockDiscontinuityContract": mailbox_contract})
    }
}

#[cfg(feature = "native-lab")]
struct LinuxGraphProcess<'a> {
    state: crate::processing::LinuxProcessState<'a>,
    counters: Arc<DriverCounters>,
}

#[cfg(feature = "native-lab")]
impl mixrelay_linux_pipewire::DriverProcess for LinuxGraphProcess<'_> {
    fn prepare_inputs(
        &mut self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), mixrelay_linux_pipewire::DriverError> {
        self.state.prepare_inputs(inputs)
    }

    fn process(&mut self, cycle: &mixrelay_linux_pipewire::DriverCycle) {
        self.state.process(cycle);
        // These test-only loads borrow current owners after their normal report
        // work. No native pointer, report baseline or PCM cursor escapes.
        let sets = [
            self.state.captures.transfer_snapshot(),
            self.state.endpoints.transfer_snapshot(),
            self.state.devices.transfer_snapshot(),
        ];
        // This exclusive DriverProcess writer publishes a fixed bundle. Markers
        // AND payload use SeqCst: ARM64 has the same SC order, with odd before
        // changed words and even after them. A reader observing changed words
        // cannot validate the old even marker. This protects owner labels, not
        // the independently observed C queue values inside the bundle.
        self.counters
            .transfer_generation
            .fetch_add(1, Ordering::SeqCst);
        self.counters.cycles.fetch_add(1, Ordering::SeqCst);
        self.counters
            .frames
            .fetch_add(u64::from(cycle.frames), Ordering::SeqCst);
        self.counters.discontinuities.store(
            self.state
                .clock_diagnostics
                .read_since(0)
                .map_or(0, |read| read.event_number),
            Ordering::SeqCst,
        );
        self.counters.clock.store(
            u64::from(cycle.clock_id) << 32 | u64::from(cycle.rate),
            Ordering::SeqCst,
        );
        for (destination, snapshot) in self.counters.transfers.iter().zip(sets) {
            for (word, value) in destination.iter().zip(snapshot.words()) {
                word.store(value, Ordering::SeqCst);
            }
        }
        self.counters
            .position
            .store(cycle.position, Ordering::SeqCst);
        self.counters
            .transfer_generation
            .fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_graph_continuity_preserves_480k_frames_through_engine() {
    let (source_path, sink_path, directory) = super::mute::fixture_paths();
    let mut evidence = GraphCaptureEvidence::new(&directory);
    let mut source = FixtureProcess::start(&source_path, &[]);
    let source_target = wait_for_owned_target(
        source.id(),
        SOURCE_APP_ID,
        SOURCE_NODE_NAME,
        "Stream/Output/Audio",
    );
    let mut independent = continuity::independent_recorder(&source_target.serial, &directory);
    let engine_identity = EngineProcessIdentity {
        process_id: std::process::id(),
        start_time: process_start_time(std::process::id()),
    };
    let process_id = engine_identity.process_id.to_string();
    let mut observer = super::mute::FixtureChild::spawn(
        &sink_path,
        &[
            "--graph-route",
            &process_id,
            &engine_identity.start_time.to_string(),
            ENGINE_OUTPUT_NODE_NAME,
        ],
        &[],
    );
    let sink_ready = observer.event("ready", None);
    let sink_node_name = verified_sink_node_name(&sink_ready, observer.id());
    let document = graph_document(source.id(), &sink_node_name);
    let result = capture_graph_window(
        &document,
        &source_target,
        &mut evidence,
        &mut observer,
        engine_identity,
    );
    let (recorder_running, recorder_status) = stop_recorder(&mut independent);
    let source_summary = continuity::finish_source(&mut source, &directory);
    let (independent_tone, independent_sequence) = continuity::independent_continuity(&directory);
    validate_graph_evidence(
        &mut evidence,
        &GraphValidation {
            directory,
            result,
            observer_id: observer.id(),
            recorder_running,
            recorder_status,
            source_summary,
            independent_tone,
            independent_sequence,
        },
    );
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state() {
    let (source_path, sink_path, directory) = super::mute::fixture_paths();
    let (_driver, mut source, endpoint_names) = start_forward_source(&directory, &source_path);
    let selected = wait_for_owned_target(
        source.id(),
        SOURCE_APP_ID,
        SOURCE_NODE_NAME,
        "Stream/Output/Audio",
    );
    let engine_identity = EngineProcessIdentity {
        process_id: std::process::id(),
        start_time: process_start_time(std::process::id()),
    };
    let composed = composed_source_mute_graph(source.id(), SINK_NODE_NAME);
    let output_name = composed_output_node_name(&composed.native_outputs, SINK_NODE_NAME);
    let mut observer = super::mute::FixtureChild::spawn(
        &sink_path,
        &[
            "--graph-route",
            &engine_identity.process_id.to_string(),
            &engine_identity.start_time.to_string(),
            &output_name,
        ],
        &[],
    );
    let sink_name = verified_sink_node_name(&observer.event("ready", None), observer.id());
    assert_eq!(sink_name, SINK_NODE_NAME);
    assert_eq!(composed.native_sources.len(), 1);
    assert_eq!(composed.playback_intents.len(), 1);
    let intent = &composed.playback_intents[0];
    assert_eq!(
        composed.native_sources.get(&intent.source_id),
        Some(&intent.capture_node_id)
    );
    let (evidence, stats) = with_engine_graph(
        &composed.document,
        RenderFormat {
            sample_rate: SAMPLE_RATE,
            block_frames: BLOCK_FRAMES,
        },
        &composed.playback_intents,
        |driver, (status, counters), backend, exchange, controls| {
            run_forward_session(&mut ForwardInputs {
                driver,
                status,
                counters,
                backend,
                exchange,
                controls,
                observer: &mut observer,
                directory: &directory,
                selected: &selected,
                endpoint_names: &endpoint_names,
                intent,
                engine_identity,
            })
        },
    );
    let source_summary = continuity::finish_source(&mut source, &directory);
    assert_eq!(source_summary["valid"], true, "source fixture PCM trace");
    assert_eq!(stats.sample_rate(), graph_clock_rate());
    assert!(
        stats.frames * u64::from(SAMPLE_RATE)
            >= WINDOW_FRAMES as u64 * u64::from(graph_clock_rate())
    );
    assert!(stats.cycles > 0);
    assert_eq!(evidence["observerExit"], true);
    let summary = serde_json::json!({
        "source": source_summary,
        "driver": {"cycles": stats.cycles, "frames": stats.frames,
            "rate": stats.sample_rate()},
        "evidence": evidence,
    });
    std::fs::write(
        directory.join(format!("source-mute-forward-{}.json", std::process::id())),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .expect("save actual Forward/WP ACK evidence");
    println!("native_source_mute_forward={summary}");
}

#[cfg(feature = "native-lab")]
fn start_forward_source(
    directory: &Path,
    source_path: &Path,
) -> (driver_fixture::DriverFixture, FixtureProcess, Vec<String>) {
    let endpoint = forward_endpoint();
    let names = mixrelay_linux_driver::native_names(&endpoint);
    assert_eq!(names.len(), 2);
    let driver = driver_fixture::DriverFixture::start(directory, endpoint);
    let source = FixtureProcess::start(source_path, &["--policy-route", &names[0]]);
    (driver, source, names)
}

#[cfg(feature = "native-lab")]
struct GraphValidation {
    directory: PathBuf,
    result: GraphWindowResult,
    observer_id: u32,
    recorder_running: bool,
    recorder_status: ExitStatus,
    source_summary: Value,
    independent_tone: ToneContinuity,
    independent_sequence: SequenceContinuity,
}

#[cfg(feature = "native-lab")]
struct GraphPcmResults {
    sink_metadata: Value,
    input_tone: ToneContinuity,
    input_sequence: SequenceContinuity,
    output_tone: ToneContinuity,
    output_sequence: SequenceContinuity,
}

#[cfg(feature = "native-lab")]
fn validate_graph_evidence(evidence: &mut GraphCaptureEvidence, run: &GraphValidation) {
    let (sink_metadata, output_frames) = load_graph_output(&run.directory, run.observer_id);
    let pcm = GraphPcmResults {
        sink_metadata,
        input_tone: measure_tone(&evidence.frames),
        input_sequence: measure_sequence(&evidence.frames),
        output_tone: measure_tone(&output_frames),
        output_sequence: measure_sequence(&output_frames),
    };
    let recorder_errors =
        std::fs::read_to_string(run.directory.join("independent-capture.stderr.log"))
            .expect("independent capture stderr evidence");
    save_graph_summary(evidence, run, &pcm, &recorder_errors);
    evidence.assert_no_native_capture_loss();

    assert!(
        run.result.observer_status.success(),
        "graph observer exit: {}",
        run.result.observer_status
    );
    assert!(
        run.recorder_running && matches!(run.recorder_status.code(), Some(0 | 1)),
        "independent source recorder did not stop cleanly: {}",
        run.recorder_status
    );
    assert!(
        recorder_errors.is_empty(),
        "source recorder stderr: {recorder_errors}"
    );
    assert_eq!(
        run.source_summary["valid"], true,
        "source trace: {}",
        run.source_summary
    );
    assert!(!run.result.engine_stopped, "engine graph requested stop");
    assert_eq!(evidence.frames.len(), WINDOW_FRAMES);
    assert!(
        run.result.stats.cycles > 0,
        "GraphDriver produced no callbacks"
    );
    assert!(
        run.result.stats.frames * u64::from(SAMPLE_RATE)
            >= WINDOW_FRAMES as u64 * u64::from(graph_clock_rate()),
        "engine clock covered too few frames"
    );
    assert_eq!(
        run.result.stats.sample_rate(),
        graph_clock_rate(),
        "engine callback rate changed"
    );
    assert_graph_pcm(&pcm, run);
}

#[cfg(feature = "native-lab")]
fn assert_graph_pcm(pcm: &GraphPcmResults, run: &GraphValidation) {
    if graph_clock_rate() == SAMPLE_RATE {
        assert_continuity(
            &pcm.input_tone,
            &pcm.input_sequence,
            "separate ProcessId CaptureSet evidence tap",
        );
        assert_continuity(
            &pcm.output_tone,
            &pcm.output_sequence,
            "AudioDeviceOutput graph route",
        );
    } else {
        assert_tone_continuity(
            &pcm.input_tone,
            "separate ProcessId CaptureSet evidence tap",
        );
        assert_tone_continuity(&pcm.output_tone, "AudioDeviceOutput graph route");
        assert_clock_window(&run.result.output_window, graph_clock_rate());
    }
    assert!(
        run.independent_tone.intact(),
        "independent source tone: {:?}",
        run.independent_tone
    );
    if graph_clock_rate() == SAMPLE_RATE {
        assert!(
            run.independent_sequence.intact(),
            "independent source frame sequence: {:?}",
            run.independent_sequence
        );
    } else {
        assert!(
            (run.independent_tone.peak - 0.125).abs() <= 0.001,
            "independent converted source tone level"
        );
    }
}

#[cfg(feature = "native-lab")]
fn save_graph_summary(
    evidence: &mut GraphCaptureEvidence,
    run: &GraphValidation,
    pcm: &GraphPcmResults,
    recorder_errors: &str,
) {
    let sequence_applicable = graph_clock_rate() == SAMPLE_RATE;
    let summary = serde_json::json!({
        "format": {"sampleRate": SAMPLE_RATE, "blockFrames": BLOCK_FRAMES},
        "measuredFrames": evidence.frames.len(),
        "sequenceApplicable": sequence_applicable,
        "sequenceContract": "exact sequence gate applies only without PipeWire SRC; \
            converted PCM is qualified by tone continuity and callback-duration/frame balance",
        "independentCaptureEvidenceTap": {
            "role": "separate ProcessId CaptureSet tap; not GraphDriver internal ApplicationCapture buffers",
            "sourceSampleFaults": evidence.source_sample_faults,
            "tone": pcm.input_tone,
            "sequence": sequence_applicable.then_some(&pcm.input_sequence),
        },
        "engineOutput": {"tone": pcm.output_tone,
            "sequence": sequence_applicable.then_some(&pcm.output_sequence)},
        "independentSourceCapture": {"tone": run.independent_tone,
            "sequence": sequence_applicable.then_some(&run.independent_sequence)},
        "engineClock": {"clockId": run.result.stats.clock >> 32,
            "rate": run.result.stats.sample_rate(),
            "callbacks": run.result.stats.cycles, "callbackFrames": run.result.stats.frames},
        "source": run.source_summary,
        "observer": pcm.sink_metadata,
        "observerExit": run.result.observer_status.to_string(),
        "independentRecorderExit": run.recorder_status.to_string(),
        "independentRecorderRunningBeforeStop": run.recorder_running,
        "independentRecorderStderr": recorder_errors,
        "route": run.result.route,
        "outputWindow": run.result.output_window,
    });
    evidence
        .save(&summary)
        .expect("save graph continuity evidence");
    println!("native_application_graph_continuity={summary}");
}

#[cfg(feature = "native-lab")]
struct GraphWindowResult {
    route: Value,
    observer_status: ExitStatus,
    stats: DriverStats,
    engine_stopped: bool,
    output_window: Value,
}

#[derive(Clone, Copy)]
#[cfg(feature = "native-lab")]
struct EngineProcessIdentity {
    process_id: u32,
    start_time: u64,
}

#[cfg(feature = "native-lab")]
fn capture_graph_window(
    document: &GraphDocument,
    source: &PipeWireTarget,
    evidence: &mut GraphCaptureEvidence,
    observer: &mut super::mute::FixtureChild,
    engine_identity: EngineProcessIdentity,
) -> GraphWindowResult {
    let ((route, observer_status, engine_stopped, output_window), stats) = with_engine_graph(
        document,
        RenderFormat {
            sample_rate: SAMPLE_RATE,
            block_frames: BLOCK_FRAMES,
        },
        &[],
        |driver, (status, counters), _, _, _| {
            wait_driver_ready(driver, status);
            let route = observer.event("routeReady", None);
            verify_route_ready(
                &route,
                engine_identity.process_id,
                engine_identity.start_time,
            );
            let before = counters.transfer_observation();
            observer.send("phase 1");
            observer.event("armed", Some(1));
            // The independent recorder already observes graph preparation. Open
            // this bounded evidence queue only when its consumer can run: leaving
            // it undrained during graph setup manufactures a 32768-frame overflow.
            let (mut input_capture, _watch) = open_input_capture(
                source.process_id.expect("owned source process"),
                &source.serial,
            );
            evidence.collect(&mut input_capture);
            let completed = observer.event("phaseComplete", Some(1));
            let after = counters.transfer_observation();
            assert_eq!(completed["frames"].as_u64(), Some(WINDOW_FRAMES as u64));
            let observer_status = stop_observer(observer);
            let stopped = status.stop_requested();
            let output_window = serde_json::json!({"before": before, "after": after});
            (route, observer_status, stopped, output_window)
        },
    );
    GraphWindowResult {
        route,
        observer_status,
        stats,
        engine_stopped,
        output_window,
    }
}

#[cfg(feature = "native-lab")]
struct DriverStats {
    cycles: u64,
    frames: u64,
    clock: u64,
}

#[cfg(feature = "native-lab")]
impl DriverStats {
    #[cfg(feature = "native-lab")]
    fn sample_rate(&self) -> u32 {
        u32::try_from(self.clock & u64::from(u32::MAX)).expect("masked clock rate fits u32")
    }
}

#[cfg(feature = "native-lab")]
fn with_engine_graph<R>(
    document: &GraphDocument,
    format: RenderFormat,
    playback_intents: &[mixrelay_audio_graph::composition::ApplicationPlaybackIntent],
    run: impl FnOnce(
        &mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'_>>,
        (
            &mixrelay_engine_core::session::AdapterRuntimeStatus,
            &DriverCounters,
        ),
        &crate::backend::LinuxVirtualEndpointBackend,
        &Arc<mixrelay_render::ProcessorExchange>,
        &Arc<mixrelay_render::RuntimeControlBank>,
    ) -> R,
) -> (R, DriverStats) {
    let backend = crate::backend::LinuxVirtualEndpointBackend::new(
        crate::test_support::test_driver(),
        format,
        Arc::new(crate::format::EndpointObservation::default()),
    );
    let processor = crate::runtime::create_initial_processor(
        document,
        mixrelay_engine_core::default_peak_meter_bank(),
        format,
    )
    .expect("compile exact ApplicationCapture to AudioDeviceOutput graph");
    let exchange = Arc::new(mixrelay_render::ProcessorExchange::new(
        processor.plan().revision(),
    ));
    let controls = Arc::clone(processor.plan().controls());
    let playback = if playback_intents.is_empty() {
        mixrelay_engine_core::PlaybackBindings::Preserve
    } else {
        mixrelay_engine_core::PlaybackBindings::Forward {
            intents: playback_intents,
            controls: &controls,
        }
    };
    backend
        .reconcile(document, playback)
        .expect("stage graph resources");
    let status = active_runtime_status(format);
    let state = prepare_graph_process_state(&backend, processor, &exchange, &status);
    let processing = status
        .begin_processing()
        .expect("start engine processing lease");
    let counters = Arc::new(DriverCounters {
        clock_diagnostics: Arc::clone(&state.clock_diagnostics),
        ..DriverCounters::default()
    });
    let driver = mixrelay_linux_pipewire::GraphDriver::open(
        LinuxGraphProcess {
            state,
            counters: Arc::clone(&counters),
        },
        mixrelay_linux_pipewire::DriverFormat::new(format.sample_rate, format.block_frames)
            .expect("valid fixed graph-driver format"),
    )
    .expect("open real Linux GraphDriver");
    let result = run(
        &driver,
        (&status, &counters),
        &backend,
        &exchange,
        &controls,
    );
    drop(driver);
    drop(processing);
    (result, read_driver_counters(&counters))
}

#[cfg(feature = "native-lab")]
fn prepare_graph_process_state<'a>(
    backend: &'a crate::backend::LinuxVirtualEndpointBackend,
    processor: Box<mixrelay_render::BlockProcessor>,
    exchange: &Arc<mixrelay_render::ProcessorExchange>,
    status: &Arc<mixrelay_engine_core::session::AdapterRuntimeStatus>,
) -> crate::processing::LinuxProcessState<'a> {
    let format = processor.plan().format();
    let mut endpoints = Box::default();
    let mut captures = Box::default();
    let mut devices = Box::default();
    backend
        .activate_pending(
            processor.plan().revision(),
            &mut endpoints,
            &mut captures,
            &mut devices,
        )
        .expect("activate exact source and sink streams");
    backend.reclaim_retired();
    let (application_mailbox, device_mailbox) = backend.recovery_mailboxes();
    let application_recovery = application_mailbox
        .try_consumer()
        .expect("unique application capture recovery consumer");
    let device_recovery = device_mailbox
        .try_consumer()
        .expect("unique device recovery consumer");
    crate::processing::LinuxProcessState {
        failure: Arc::new(crate::processing::failure::FailureMailbox::default()),
        resource_failures: Arc::new(crate::processing::failure::FailureMailbox::default()),
        processor,
        exchange: Arc::clone(exchange),
        endpoint_backend: backend,
        application_recovery,
        device_recovery,
        endpoints,
        captures,
        devices,
        status: Arc::clone(status),
        timeline: crate::processing::clock::GraphTimeline::new(format.sample_rate)
            .expect("valid graph timeline"),
        clock_diagnostics: Arc::default(),
    }
}

#[cfg(feature = "native-lab")]
fn active_runtime_status(
    format: RenderFormat,
) -> Arc<mixrelay_engine_core::session::AdapterRuntimeStatus> {
    let status = Arc::new(mixrelay_engine_core::session::AdapterRuntimeStatus::new(
        crate::runtime::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    status.activate_format(format);
    status
}

#[cfg(feature = "native-lab")]
fn read_driver_counters(counters: &DriverCounters) -> DriverStats {
    DriverStats {
        cycles: counters.cycles.load(Ordering::Acquire),
        frames: counters.frames.load(Ordering::Acquire),
        clock: counters.clock.load(Ordering::Acquire),
    }
}

#[cfg(feature = "native-lab")]
fn open_input_capture(process_id: u32, source_serial: &str) -> (CaptureSet, CaptureWatch) {
    let request = application_capture_request(process_id);
    let format = RenderFormat {
        sample_rate: SAMPLE_RATE,
        block_frames: BLOCK_FRAMES,
    };
    let (mut captures, mut watch) = CaptureSet::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &[(CAPTURE_NODE.into(), request)],
        1,
        format,
        None,
    )
    .expect("open fixed-format input evidence tap");
    super::recover_until(&mut captures, &mut watch, CAPTURE_NODE, true);
    assert_eq!(watch.entries.len(), 1);
    assert_eq!(watch.entries[0].serials, [source_serial]);
    (captures, watch)
}

fn application_capture_request(process_id: u32) -> ApplicationCaptureRequest {
    ApplicationCaptureRequest {
        selector: ApplicationSelector::ProcessId {
            process_id,
            include_descendants: false,
        },
        stream: StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        },
        source_playback: SourcePlaybackPolicy::Preserve,
        forwarding_endpoint: None,
        gain_db: 0.0,
        requires_reselection: false,
    }
}

fn graph_document(process_id: u32, sink_node_name: &str) -> GraphDocument {
    let capture = application_capture_request(process_id);
    GraphDocument::new(
        1,
        vec![
            NodeSpec {
                id: "application".into(),
                label: Some("owned graph continuity source".into()),
                control: mixrelay_audio_graph::NodeControl::default(),
                config: NodeConfig::ApplicationCapture(capture),
            },
            NodeSpec {
                id: "engine-output".into(),
                label: Some("owned graph continuity sink".into()),
                control: mixrelay_audio_graph::NodeControl::default(),
                config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: sink_node_name.into(),
                    },
                    input_channels: 2,
                }),
            },
        ],
        vec![EdgeSpec {
            id: "application-to-sink".into(),
            source: AudioEndpoint {
                node_id: "application".into(),
                port_id: AUDIO_PORT_ID.into(),
            },
            destination: AudioEndpoint {
                node_id: "engine-output".into(),
                port_id: AUDIO_PORT_ID.into(),
            },
            channel_mapping: ChannelMapping::default(),
        }],
    )
}

struct NativeApplicationResolver;

impl ApplicationIdentityResolver for NativeApplicationResolver {
    fn resolve(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<SharedSource, CompositionError> {
        crate::applications::identity::resolve(request)
            .map_err(|error| CompositionError::Invalid(error.to_string()))
    }
}

fn forward_endpoint() -> VirtualEndpointSpec {
    VirtualEndpointSpec {
        id: FORWARD_ENDPOINT_ID.into(),
        display_name: "MixRelay private source-mute target".into(),
        render: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        capture: Some(StreamSpec {
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
        }),
        supported_sample_rates: vec![SAMPLE_RATE],
        default_sample_rate: SAMPLE_RATE,
    }
}

fn source_mute_graph(process_id: u32, sink_node_name: &str) -> GraphDocument {
    let mut document = graph_document(process_id, sink_node_name);
    let NodeConfig::ApplicationCapture(request) = &mut document.nodes[0].config else {
        unreachable!("graph helper always creates application capture")
    };
    request.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    request.forwarding_endpoint = Some(FORWARD_ENDPOINT_ID.into());
    document.nodes.push(NodeSpec {
        id: "owned-forward-endpoint".into(),
        label: Some("explicit private forwarding endpoint".into()),
        control: mixrelay_audio_graph::NodeControl::default(),
        config: NodeConfig::VirtualEndpoint(VirtualEndpointConfig {
            spec: forward_endpoint(),
            reference: VirtualEndpointReference {
                render: true,
                capture: true,
            },
            gain_db: 0.0,
        }),
    });
    document
}

fn composed_source_mute_graph(
    source_pid: u32,
    sink_node_name: &str,
) -> mixrelay_audio_graph::composition::ComposedGraph {
    let document = source_mute_graph(source_pid, sink_node_name);
    let devices = [AudioDeviceDescriptor {
        uid: sink_node_name.into(),
        display_name: "private graph observer".into(),
        input_channels: 0,
        output_channels: 2,
        is_default_input: false,
        is_default_output: false,
    }];
    compose_catalog(
        &[("source-mute".into(), document)],
        CatalogExecutionContext {
            revision: 1,
            compile: CompileContext {
                format: RenderFormat {
                    sample_rate: SAMPLE_RATE,
                    block_frames: BLOCK_FRAMES,
                },
                limits: CompileLimits::default(),
            },
            compiler: &GraphCompiler::default(),
            devices: &devices,
            applications: &NativeApplicationResolver,
        },
    )
    .expect("compose authored Forward intent with one canonical capture")
}

fn composed_output_node_name(outputs: &BTreeMap<String, NodeId>, sink_node_name: &str) -> String {
    assert_eq!(
        outputs.len(),
        1,
        "composed graph must have one private sink output"
    );
    let native_output_key = format!("device:{}:{sink_node_name}", sink_node_name.len());
    let output_id = outputs
        .get(&native_output_key)
        .expect("composed graph retains the selected private sink output");
    format!(
        "{}.device-output",
        crate::runtime::sanitize_node_name(output_id.as_str())
    )
}

#[cfg(feature = "native-lab")]
fn registry_snapshot() -> Vec<Value> {
    let output =
        mixrelay_linux_audio_io::bounded_pipewire_output("pw-dump", &[], Duration::from_secs(2))
            .expect("bounded private PipeWire registry read");
    assert!(output.status.success(), "private pw-dump failed");
    serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter::<Vec<Value>>()
        .flat_map(|batch| batch.expect("valid private PipeWire registry batch"))
        .collect()
}

#[cfg(feature = "native-lab")]
fn metadata_value(objects: &[Value], name: &str, subject: u32, key: &str) -> Option<String> {
    objects
        .iter()
        .find(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == name
        })?
        .get("metadata")?
        .as_array()?
        .iter()
        .find(|entry| entry["subject"].as_u64() == Some(u64::from(subject)) && entry["key"] == key)?
        .get("value")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(feature = "native-lab")]
fn default_metadata(objects: &[Value]) -> std::collections::BTreeMap<(u32, String), Value> {
    let entries = objects
        .iter()
        .find(|object| {
            object["type"] == "PipeWire:Interface:Metadata"
                && object["props"]["metadata.name"] == "default"
        })
        .and_then(|object| object["metadata"].as_array())
        .expect("private WirePlumber default metadata");
    entries
        .iter()
        .map(|entry| {
            (
                (
                    u32::try_from(entry["subject"].as_u64().expect("metadata subject")).unwrap(),
                    entry["key"].as_str().expect("metadata key").to_owned(),
                ),
                entry.clone(),
            )
        })
        .collect()
}

fn private_target_args<'a>(source: &'a str, target: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["-n", "default", source, "target.object"];
    if let Some(target) = target {
        args.extend([target, "Spa:String"]);
    } else {
        args.insert(0, "-d");
    }
    args
}

#[test]
fn private_target_restore_deletes_only_the_exact_private_key() {
    assert_eq!(
        private_target_args("41", Some("io.mixrelay.fixture.policy-reroute-sink")),
        [
            "-n",
            "default",
            "41",
            "target.object",
            "io.mixrelay.fixture.policy-reroute-sink",
            "Spa:String"
        ]
    );
    assert_eq!(
        private_target_args("41", None),
        ["-d", "-n", "default", "41", "target.object"]
    );
}

#[cfg(feature = "native-lab")]
fn update_private_target(source: u32, target: Option<&str>) {
    let source = source.to_string();
    let args = private_target_args(&source, target);
    let output = mixrelay_linux_audio_io::bounded_pipewire_output(
        "pw-metadata",
        &args,
        Duration::from_secs(2),
    )
    .expect("bounded private playback target change");
    assert!(
        output.status.success(),
        "private playback target change failed"
    );
}

#[cfg(feature = "native-lab")]
fn wait_for_ack(
    backend: &crate::backend::LinuxVirtualEndpointBackend,
    exchange: &mixrelay_render::ProcessorExchange,
    source_serial: &str,
    source_id: u32,
    endpoint_id: u32,
) -> String {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    let lease_key = format!("lease.{source_serial}");
    let ack_key = format!("ack.{source_serial}");
    loop {
        backend.poll_source_mute(exchange);
        let objects = registry_snapshot();
        let lease = metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &lease_key);
        let ack = metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &ack_key);
        if let (Some(lease), Some(ack)) = (lease, ack)
            && lease == ack
            && managed_links(&objects, source_id, endpoint_id).is_empty()
        {
            return lease;
        }
        assert!(
            Instant::now() < deadline,
            "exact WP ACK/leased-link removal absent"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
fn wait_for_lease_clear(
    backend: &crate::backend::LinuxVirtualEndpointBackend,
    exchange: &mixrelay_render::ProcessorExchange,
    source_serial: &str,
    endpoint_id: u32,
) {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    let lease_key = format!("lease.{source_serial}");
    let ack_key = format!("ack.{source_serial}");
    loop {
        backend.poll_source_mute(exchange);
        let objects = registry_snapshot();
        if metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &lease_key).is_none()
            && metadata_value(&objects, "mixrelay-source-mute", endpoint_id, &ack_key).is_none()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "exact WP lease/ACK did not clear"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
fn managed_links(objects: &[Value], source: u32, sink: u32) -> Vec<u32> {
    objects
        .iter()
        .filter(|object| {
            object["type"] == "PipeWire:Interface:Link"
                && object["info"]["output-node-id"].as_u64() == Some(u64::from(source))
                && object["info"]["input-node-id"].as_u64() == Some(u64::from(sink))
        })
        .map(|link| {
            let client_id = link["info"]["props"]["client.id"]
                .as_u64()
                .expect("managed link has server-bound client ID");
            let client = objects
                .iter()
                .find(|object| object["id"].as_u64() == Some(client_id))
                .expect("managed link has live client");
            assert_eq!(client["type"], "PipeWire:Interface:Client");
            // SAFETY: geteuid reads only this process credential, with no pointer access.
            let uid = unsafe { libc::geteuid() };
            assert_eq!(
                client["info"]["props"]["pipewire.sec.uid"].as_u64(),
                Some(u64::from(uid))
            );
            let pid = u32::try_from(
                client["info"]["props"]["pipewire.sec.pid"]
                    .as_u64()
                    .expect("server client PID"),
            )
            .unwrap();
            let executable = mixrelay_linux_audio_io::process_identity::process_identity(pid)
                .and_then(|identity| identity.executable_path)
                .expect("live link client executable");
            assert_eq!(
                Path::new(&executable)
                    .file_name()
                    .and_then(|name| name.to_str()),
                Some("wireplumber")
            );
            u32::try_from(link["id"].as_u64().expect("link ID")).unwrap()
        })
        .collect()
}

#[cfg(feature = "native-lab")]
fn wait_for_managed_links(source: u32, sink: u32, expected: usize) -> Vec<u32> {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let registry = registry_snapshot();
        let links = managed_links(&registry, source, sink);
        if links.len() == expected {
            return links;
        }
        if Instant::now() >= deadline {
            let fixture = std::env::var_os("MIXRELAY_LAB_APPLICATION_FIXTURE")
                .map(PathBuf::from)
                .expect("explicit owned fixture path");
            let evidence = fixture.parent().unwrap().join(format!(
                "managed-route-timeout-{source}-{sink}-{expected}.json"
            ));
            let output = File::create_new(&evidence).expect("exclusive failed-route evidence");
            serde_json::to_writer(output, &registry).expect("save failed-route registry");
            panic!(
                "WP-managed route count did not become {expected}; found {links:?}; evidence={}",
                evidence.display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
fn capture_owned_endpoint(serial: &str, directory: &Path, label: &str) -> Vec<f32> {
    let raw_path = directory.join(format!("source-mute-{label}.f32"));
    let log_path = directory.join(format!("source-mute-{label}.log"));
    let output = File::create_new(&raw_path).expect("new owned endpoint PCM evidence");
    let errors = File::create_new(log_path).expect("new owned recorder log");
    let child = Command::new("pw-record")
        .args([
            "--raw",
            "--rate=48000",
            "--channels=2",
            "--format=f32",
            "--channel-map=stereo",
            "--target",
            serial,
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors)
        .spawn()
        .expect("private owned-endpoint PCM recorder");
    let mut recorder = FixtureProcess(child);
    thread::sleep(Duration::from_secs(3));
    recorder.stop();
    let bytes = std::fs::read(raw_path).expect("recorded owned-endpoint PCM");
    assert_eq!(bytes.len() % 8, 0, "stereo f32 recorder alignment");
    assert!(bytes.len() >= 48_000 * 8, "owned-endpoint PCM too short");
    bytes
        .chunks_exact(8)
        .map(|frame| f32::from_ne_bytes(frame[..4].try_into().unwrap()))
        .collect()
}

#[cfg(feature = "native-lab")]
fn tone_amplitude(samples: &[f32], frequency: f64) -> f64 {
    let window = &samples[samples.len() - 24_000..];
    let mut sine = 0.0;
    let mut cosine = 0.0;
    for (frame, sample) in window.iter().enumerate() {
        let frame = u32::try_from(frame).expect("fixed 24,000-frame tone window");
        let phase = std::f64::consts::TAU * frequency * f64::from(frame) / f64::from(SAMPLE_RATE);
        sine += f64::from(*sample) * phase.sin();
        cosine += f64::from(*sample) * phase.cos();
    }
    let window_len = u32::try_from(window.len()).expect("fixed 24,000-frame tone window");
    2.0 * sine.hypot(cosine) / f64::from(window_len)
}

#[cfg(feature = "native-lab")]
fn unrelated_tone_file(directory: &Path) -> PathBuf {
    let path = directory.join("source-mute-unrelated-880.f32");
    let mut output = BufWriter::new(File::create_new(&path).expect("owned 880 Hz test media"));
    for frame in 0..(SAMPLE_RATE * 40) {
        let phase = std::f64::consts::TAU * 880.0 * f64::from(frame) / f64::from(SAMPLE_RATE);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the bounded sine is intentionally rounded to f32 PCM"
        )]
        let sample = (0.1 * phase.sin() as f32).to_le_bytes();
        output.write_all(&sample).unwrap();
        output.write_all(&sample).unwrap();
    }
    output.flush().expect("flush owned 880 Hz test media");
    path
}

#[cfg(feature = "native-lab")]
fn start_unrelated_tone(serial: &str, directory: &Path) -> FixtureProcess {
    let media = unrelated_tone_file(directory);
    let input = File::open(media).expect("read owned unrelated tone media");
    let log = File::create_new(directory.join("source-mute-unrelated-880.log"))
        .expect("new unrelated tone log");
    let props = format!(
        r#"{{"application.id":"{UNRELATED_APP_ID}","application.name":"MixRelay unrelated 880 Hz fixture"}}"#
    );
    FixtureProcess(
        Command::new("pw-play")
            .args([
                "--raw",
                "--rate=48000",
                "--channels=2",
                "--format=f32",
                "--channel-map=stereo",
                "--properties",
                &props,
                "--target",
                serial,
                "-",
            ])
            .stdin(input)
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("private unrelated 880 Hz pw-play"),
    )
}

#[cfg(feature = "native-lab")]
struct ForwardInputs<'a, 'b> {
    driver: &'a mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'b>>,
    status: &'a mixrelay_engine_core::session::AdapterRuntimeStatus,
    counters: &'a DriverCounters,
    backend: &'a crate::backend::LinuxVirtualEndpointBackend,
    exchange: &'a Arc<mixrelay_render::ProcessorExchange>,
    controls: &'a Arc<mixrelay_render::RuntimeControlBank>,
    observer: &'a mut super::mute::FixtureChild,
    directory: &'a Path,
    selected: &'a mixrelay_linux_audio_io::PipeWireTarget,
    endpoint_names: &'a [String],
    intent: &'a mixrelay_audio_graph::composition::ApplicationPlaybackIntent,
    engine_identity: EngineProcessIdentity,
}

#[cfg(feature = "native-lab")]
fn wait_for_playback_target(input: &ForwardInputs<'_, '_>) {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let watch = input
            .backend
            .application_watch()
            .expect("active graph capture watch");
        let inventory = mixrelay_linux_audio_io::targets().expect("private target inventory");
        let targets =
            watch.verified_playback_targets(input.intent.capture_node_id.as_str(), &inventory);
        if targets.len() == 1 && targets[0].serial == input.selected.serial {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "pre-mute graph PCM was not ready"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
fn wait_for_unrelated_output(process_id: u32) -> mixrelay_linux_audio_io::PipeWireTarget {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let candidates = mixrelay_linux_audio_io::targets()
            .expect("private PipeWire target inventory")
            .into_iter()
            .filter(|target| {
                target.process_id == Some(process_id)
                    && target.application_id.as_deref() == Some(UNRELATED_APP_ID)
                    && target.media_class == "Stream/Output/Audio"
            })
            .collect::<Vec<_>>();
        assert!(candidates.len() <= 1, "ambiguous unrelated 880 Hz output");
        if let Some(target) = candidates.into_iter().next() {
            return target;
        }
        assert!(
            Instant::now() < deadline,
            "unrelated 880 Hz output not ready"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(feature = "native-lab")]
fn publish_source_mask(input: &ForwardInputs<'_, '_>, revision: u64, enabled: bool) {
    input
        .exchange
        .publish_controls(
            input.controls,
            revision,
            &[mixrelay_render::RuntimeControlUpdate {
                node_id: input.intent.mask_node_id.clone(),
                control: mixrelay_audio_graph::NodeControl {
                    enabled,
                    muted: false,
                },
                gain_db: None,
            }],
        )
        .expect("publish graph-local source playback mask in same active bank");
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while input.controls.applied_revision() < revision {
        assert!(
            Instant::now() < deadline,
            "graph-local control was not applied"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native-lab")]
fn run_forward_session(input: &mut ForwardInputs<'_, '_>) -> Value {
    let transfer_before = input.counters.transfer_observation();
    wait_driver_ready(input.driver, input.status);
    let route = input.observer.event("routeReady", None);
    verify_route_ready(
        &route,
        input.engine_identity.process_id,
        input.engine_identity.start_time,
    );
    let (render, capture) = driver_endpoint_targets(input);
    let original_links = wait_for_managed_links(input.selected.object_id, render.object_id, 2);
    let before = capture_owned_endpoint(&capture.serial, input.directory, "before");
    let before_1000 = tone_amplitude(&before, 1000.0);
    let before_has_tone = before_1000 > 0.05;
    if !before_has_tone {
        let output_window = serde_json::json!({
            "phase": "beforeLease",
            "oneKHz": before_1000,
            "snapshot": input.counters.transfer_observation(),
        });
        save_forward_transfer_observation(input, &transfer_before, &output_window, None);
    }
    assert!(
        before_1000 > 0.05,
        "selected 1 kHz was absent from X before lease"
    );
    wait_for_playback_target(input);

    let mut unrelated = start_unrelated_tone(&render.serial, input.directory);
    let witness = wait_for_unrelated_output(unrelated.id());
    // WirePlumber converts a command-line target that is also the default into
    // follow-default routing. Pin this witness as an explicit user-selected
    // route before introducing another sink, independently of the leased app.
    update_private_target(witness.object_id, Some(&render.serial));
    let witness_links = wait_for_managed_links(witness.object_id, render.object_id, 2);
    let ack = wait_for_ack(
        input.backend,
        input.exchange,
        &input.selected.serial,
        input.selected.object_id,
        render.object_id,
    );
    let muted = capture_owned_endpoint(&capture.serial, input.directory, "leased");
    let leased_1000 = tone_amplitude(&muted, 1000.0);
    let leased_880 = tone_amplitude(&muted, 880.0);
    assert!(leased_1000 < 0.005, "selected 1 kHz leaked to leased X");
    assert!(leased_880 > 0.04, "unrelated 880 Hz was muted at X");
    let output_transfer = collect_forward_graph_pcm(input);

    let off = verify_off_restored(input, &render, &capture, &transfer_before, &output_transfer);

    publish_source_mask(input, 3, true);
    let second_ack = wait_for_ack(
        input.backend,
        input.exchange,
        &input.selected.serial,
        input.selected.object_id,
        render.object_id,
    );
    let reroute = verify_external_reroute(input, &render, &capture, &witness);
    let (pre_mute_graph_output, observer_exit) =
        finish_forward_session(input, &mut unrelated, &transfer_before, &output_transfer);
    serde_json::json!({
        "route": route,
        "selectedSource": {"serial": input.selected.serial, "nodeId": input.selected.object_id},
        "endpoint": {"renderSerial": render.serial, "captureSerial": capture.serial},
        "before": {"oneKHz": before_1000, "links": original_links},
        "leased": {"oneKHz": leased_1000, "eightEightyHz": leased_880,
            "ack": ack, "witnessLinks": witness_links},
        "off": off,
        "reenabledAck": second_ack,
        "externalReroute": reroute,
        "preMuteGraphOutput": pre_mute_graph_output,
        "observerExit": observer_exit,
    })
}

#[cfg(feature = "native-lab")]
fn driver_endpoint_targets(
    input: &ForwardInputs<'_, '_>,
) -> (
    mixrelay_linux_audio_io::PipeWireTarget,
    mixrelay_linux_audio_io::PipeWireTarget,
) {
    let owner = input
        .backend
        .driver_snapshot()
        .unwrap()
        .owner_process_id
        .expect("the private Driver owns its native endpoints");
    (
        wait_for_owned_target(
            owner,
            "io.mixrelay.driver",
            &input.endpoint_names[0],
            "Audio/Sink",
        ),
        wait_for_owned_target(
            owner,
            "io.mixrelay.driver",
            &input.endpoint_names[1],
            "Audio/Source",
        ),
    )
}

#[cfg(feature = "native-lab")]
fn finish_forward_session(
    input: &mut ForwardInputs<'_, '_>,
    unrelated: &mut FixtureProcess,
    transfer_before: &Value,
    output_transfer: &Value,
) -> (Value, bool) {
    let observer_status = stop_observer(&mut *input.observer);
    save_forward_transfer_observation(input, transfer_before, output_transfer, None);
    assert!(
        observer_status.success(),
        "graph observer: {observer_status}"
    );
    let (output_metadata, output_frames) = load_graph_output(input.directory, input.observer.id());
    let output_tone = measure_tone(&output_frames);
    let output_sequence = measure_sequence(&output_frames);
    let sequence_applicable = graph_clock_rate() == SAMPLE_RATE;
    let output_pcm = serde_json::json!({"tone": output_tone,
        "sequence": sequence_applicable.then_some(&output_sequence),
        "sequenceApplicable": sequence_applicable,
        "sequenceContract": "sequence gate requires unconverted PCM; SRC mode uses tone continuity",
        "observer": output_metadata});
    save_forward_transfer_observation(input, transfer_before, output_transfer, Some(&output_pcm));
    if graph_clock_rate() == SAMPLE_RATE {
        assert_continuity(&output_tone, &output_sequence, "pre-mute graph output Y");
    } else {
        assert_tone_continuity(&output_tone, "converted pre-mute graph output Y");
        assert_clock_window(output_transfer, graph_clock_rate());
    }
    unrelated.stop();
    assert!(
        !input.status.stop_requested(),
        "engine graph requested stop"
    );
    (output_pcm, observer_status.success())
}

#[cfg(feature = "native-lab")]
fn save_forward_transfer_observation(
    input: &ForwardInputs<'_, '_>,
    before: &Value,
    output_transfer: &Value,
    output_pcm: Option<&Value>,
) {
    let observation = serde_json::json!({
        "processId": input.engine_identity.process_id,
        "processBirth": input.engine_identity.start_time,
        "diagnosticOnly": true,
        "snapshotContract": "generation-validated test owner publication; unavailable after 3 attempts; \
            enclosed C queue observations are independent atomic loads, not a queue transaction",
        "failureStages": ["nativeCaptureWrite", "nativeOutputRead",
            "engineCaptureRead", "engineOutputWrite"],
        "captureFailureContract": "nativeCaptureWrite includes malformed ingress/format/geometry \
            discards and ring admission failure; unknown frame geometry increments calls with zero \
            known frames; ordinary dequeue-null and declared empty chunks are not rejected writes",
        "counterContract": "stream lifetime; unsigned wrapping differences only for streams=1 \
            and the same nonzero owner_sequence; owner replacement starts at zero; no restart reset",
        "cursorContract": "read includes delivery, explicit discard, epoch-start cursor skips \
            and cursor commits invalidated by a concurrent restart; \
            engine_read_frames counts successful engine delivery; consumer_discard_calls includes \
            empty queues; consumer_discard_frames counts explicit discard cursor advance; \
            native_restart_calls counts state/format epoch requests; independently observed deltas \
            are not an exact transaction or an exact PCM window",
        "positionContract": "recent native IO copy; packed clock ID high32/rate low32; \
            zero clock unavailable; join only the same clock/rate and streams=1; \
            multi-stream cursor/count sums are not one queue; positions from first enumerated owner",
        "geometryContract": "native_clock and native_quantum describe the first current owner; \
            quantum packs graph frames high32/native PCM frames low32; independent observations, \
            not failure-time geometry; never attribute multi-owner losses to that first owner",
        "beforeBoundary": "control entry before Forward waits, not an exact Y window boundary",
        "afterBoundary": "control at receipt publication before PCM assertions; normal completion \
            follows observer EOF, but a failed earlier phase is saved before teardown",
        "before": before,
        "after": input.counters.transfer_observation(),
        "outputWindow": output_transfer,
        "outputPcm": output_pcm,
    });
    let path = input.directory.join(format!(
        "source-mute-transfer-{}.json",
        input.engine_identity.process_id
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(output_pcm.is_none())
        .truncate(output_pcm.is_some())
        .open(path)
        .expect("create native transfer observation before PCM assertion");
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &observation)
        .expect("save native transfer observation before PCM assertion");
    writer.flush().expect("flush native transfer observation");
}

#[test]
fn transfer_observation_retains_original_latest_clock_transition_separately() {
    let counters = DriverCounters::default();
    assert!(counters.transfer_observation()["latestClockDiscontinuity"].is_null());
    let previous = mixrelay_linux_pipewire::DriverCycle {
        clock_id: 26,
        rate: 48_000,
        position: 4096,
        frames: 128,
        monotonic_ns: 1_000_000,
    };
    let current = mixrelay_linux_pipewire::DriverCycle {
        clock_id: 27,
        rate: 44_100,
        position: 4480,
        frames: 256,
        monotonic_ns: 9_000_000,
    };
    let event = crate::processing::clock::NativeDiscontinuity {
        previous,
        current,
        mapped_epoch: 3,
        render_position: 8192,
    };
    counters
        .clock_diagnostics
        .publish(crate::processing::clock::NativeDiscontinuity {
            previous: mixrelay_linux_pipewire::DriverCycle {
                position: 3840,
                ..previous
            },
            current: previous,
            mapped_epoch: 2,
            render_position: 8064,
        });
    counters.clock_diagnostics.publish(event);
    let observation = counters.transfer_observation();
    assert_eq!(
        observation["latestClockDiscontinuity"],
        serde_json::json!({
            "previous": {"clock_id": 26, "rate": 48_000, "position": 4096,
                "frames": 128, "monotonic_ns": 1_000_000},
            "current": {"clock_id": 27, "rate": 44_100, "position": 4480,
                "frames": 256, "monotonic_ns": 9_000_000},
            "mapped_epoch": 3, "render_position": 8192,
            "event_number": 2, "overwritten": 1,
        })
    );
    assert_eq!(observation["generation"], 0);
    assert_eq!(observation["engineClock"]["discontinuities"], 0);
    assert_eq!(
        counters.transfer_observation()["latestClockDiscontinuity"],
        observation["latestClockDiscontinuity"]
    );
    counters.transfer_generation.store(1, Ordering::SeqCst);
    let unavailable = counters.transfer_observation();
    assert_eq!(unavailable["available"], false);
    assert_eq!(
        unavailable["latestClockDiscontinuity"],
        observation["latestClockDiscontinuity"]
    );
}

#[test]
fn transfer_observation_keeps_owned_sets_and_engine_clock_separate() {
    let counters = DriverCounters::default();
    let empty = counters.transfer_observation();
    assert_eq!(empty["capture"]["streams"], 0);
    for (set_index, set) in counters.transfers.iter().enumerate() {
        set[0].store(1, Ordering::Relaxed);
        set[4].store(set_index as u64 + 2, Ordering::Relaxed);
        set[8].store((10 + set_index as u64) << 32 | 0xbb80, Ordering::Relaxed);
        set[28].store(set_index as u64 + 1, Ordering::Relaxed);
        set[29].store((20 + set_index as u64) << 32 | 0xbb80, Ordering::Relaxed);
        set[30].store(256_u64 << 32 | 128, Ordering::Relaxed);
        set[31].store(1024, Ordering::Relaxed);
        set[32].store(2, Ordering::Relaxed);
        set[33].store(128, Ordering::Relaxed);
        set[34].store(3, Ordering::Relaxed);
    }
    counters
        .clock
        .store(99_u64 << 32 | 0xbb80, Ordering::Release);
    counters.position.store(4096, Ordering::Relaxed);
    counters.frames.store(8192, Ordering::SeqCst);
    counters.discontinuities.store(2, Ordering::SeqCst);
    let observation = counters.transfer_observation();
    for (index, name) in ["capture", "endpoint", "device"].iter().enumerate() {
        assert_eq!(observation[name]["streams"], 1);
        assert_eq!(observation[name]["owner_sequence"], index as u64 + 1);
        assert_eq!(
            observation[name]["native_clock"],
            (20 + index as u64) << 32 | 0xbb80
        );
        assert_eq!(observation[name]["native_quantum"], 256_u64 << 32 | 128);
        assert_eq!(observation[name]["counters"]["engine_read_frames"], 1024);
        assert_eq!(observation[name]["counters"]["consumer_discard_calls"], 2);
        assert_eq!(
            observation[name]["counters"]["consumer_discard_frames"],
            128
        );
        assert_eq!(observation[name]["counters"]["native_restart_calls"], 3);
        assert_eq!(
            observation[name]["counters"]["failures"][0]["rejected_calls"],
            index as u64 + 2
        );
        assert_ne!(
            observation[name]["counters"]["failures"][0]["first_clock"],
            99_u64 << 32 | 0xbb80
        );
    }
    assert_eq!(observation["engineClock"]["id"], 99);
    assert_eq!(observation["engineClock"]["rate"], 48_000);
    assert_eq!(observation["engineClock"]["position"], 4096);
    assert_eq!(observation["engineClock"]["callbackFrames"], 8192);
    assert_eq!(observation["engineClock"]["discontinuities"], 2);
    let mut published = false;
    let fresh = counters.transfer_observation_with(|| {
        if !published {
            counters.transfer_generation.fetch_add(1, Ordering::SeqCst);
            counters.frames.store(16_384, Ordering::SeqCst);
            counters.transfer_generation.fetch_add(1, Ordering::SeqCst);
            published = true;
        }
    });
    assert_eq!(fresh["attempts"], 2);
    assert_eq!(fresh["engineClock"]["callbackFrames"], 16_384);
}

#[test]
fn converted_clock_window_keeps_fraction_and_excludes_lifetime_failures() {
    let rate = 44_100;
    let counters = DriverCounters::default();
    counters
        .clock
        .store(7_u64 << 32 | u64::from(rate), Ordering::SeqCst);
    let output = &counters.transfers[2];
    output[0].store(1, Ordering::SeqCst);
    output[28].store(3, Ordering::SeqCst);
    output[29].store(7_u64 << 32 | u64::from(rate), Ordering::SeqCst);
    // Warm-up failures must not be charged again to the settled PCM window.
    output[10].store(5, Ordering::SeqCst);
    output[11].store(256, Ordering::SeqCst);
    counters.frames.store(128, Ordering::SeqCst);
    output[2].store(139, Ordering::SeqCst);
    let before = counters.transfer_observation();
    counters.frames.store(256, Ordering::SeqCst);
    output[2].store(278, Ordering::SeqCst);
    let after = counters.transfer_observation();
    let mut window = serde_json::json!({"before": before, "after": after});
    assert_clock_window(&window, rate);
    // The next carried fraction may legitimately produce 140 instead of 139.
    window["after"]["device"]["counters"]["written"] = serde_json::json!(279);
    assert_clock_window(&window, rate);
    window["after"]["device"]["counters"]["written"] = serde_json::json!(407);
    assert!(std::panic::catch_unwind(|| assert_clock_window(&window, rate)).is_err());
    window["after"]["device"]["counters"]["written"] = serde_json::json!(278);
    window["after"]["device"]["counters"]["native_restart_calls"] = serde_json::json!(1);
    assert!(std::panic::catch_unwind(|| assert_clock_window(&window, rate)).is_err());
}

#[cfg(feature = "native-lab")]
fn verify_off_restored(
    input: &ForwardInputs<'_, '_>,
    render: &mixrelay_linux_audio_io::PipeWireTarget,
    capture: &mixrelay_linux_audio_io::PipeWireTarget,
    transfer_before: &Value,
    output_transfer: &Value,
) -> Value {
    let off_before = input.counters.transfer_observation();
    publish_source_mask(input, 2, false);
    wait_for_lease_clear(
        input.backend,
        input.exchange,
        &input.selected.serial,
        render.object_id,
    );
    let restored_links = wait_for_managed_links(input.selected.object_id, render.object_id, 2);
    let restored = capture_owned_endpoint(&capture.serial, input.directory, "off-restored");
    let off_1000 = tone_amplitude(&restored, 1000.0);
    let off_880 = tone_amplitude(&restored, 880.0);
    if !(off_1000 > 0.05 && off_880 > 0.04) {
        let mut failed_transfer = output_transfer.clone();
        failed_transfer["offRestore"] = serde_json::json!({
            "boundaryContract": "control snapshot before Off publication and after the owned \
                recorder exits; includes lease/link restoration and recorder setup/exit delay, \
                not exact PCM callback boundaries",
            "before": off_before,
            "after": input.counters.transfer_observation(),
            "oneKHz": off_1000,
            "eightEightyHz": off_880,
            "restoredLinks": restored_links,
            "recorderPcm": input.directory.join("source-mute-off-restored.f32"),
            "recorderLog": input.directory.join("source-mute-off-restored.log"),
        });
        save_forward_transfer_observation(input, transfer_before, &failed_transfer, None);
    }
    assert!(
        off_1000 > 0.05 && off_880 > 0.04,
        "Off did not restore X only: 1 kHz={off_1000}, 880 Hz={off_880}"
    );
    serde_json::json!({
        "oneKHz": off_1000,
        "eightEightyHz": off_880,
        "restoredLinks": restored_links,
    })
}

#[cfg(feature = "native-lab")]
fn verify_external_reroute(
    input: &ForwardInputs<'_, '_>,
    render: &mixrelay_linux_audio_io::PipeWireTarget,
    capture: &mixrelay_linux_audio_io::PipeWireTarget,
    witness: &mixrelay_linux_audio_io::PipeWireTarget,
) -> Value {
    let sink_path = input.directory.join("mute-sink-fixture");
    let mut external = super::mute::FixtureChild::spawn(&sink_path, &["--policy-sink"], &[]);
    let ready = external.event("ready", None);
    assert_eq!(ready["routeMode"], "policy-sink");
    let new_sink = wait_for_owned_target(
        external.id(),
        SINK_APP_ID,
        "io.mixrelay.fixture.policy-reroute-sink",
        "Audio/Sink",
    );
    assert_eq!(
        ready["nodeId"].as_u64(),
        Some(u64::from(new_sink.object_id))
    );
    assert_eq!(ready["nodeSerial"], new_sink.serial);
    let before = registry_snapshot();
    let defaults = default_metadata(&before);
    let original_target = metadata_value(
        &before,
        "default",
        input.selected.object_id,
        "target.object",
    );
    let target_name = new_sink
        .node_name
        .as_deref()
        .expect("verified policy sink name");
    update_private_target(input.selected.object_id, Some(target_name));
    wait_for_lease_clear(
        input.backend,
        input.exchange,
        &input.selected.serial,
        render.object_id,
    );
    let moved_links = wait_for_managed_links(input.selected.object_id, new_sink.object_id, 2);
    assert_eq!(
        managed_links(&registry_snapshot(), witness.object_id, render.object_id).len(),
        2,
        "external reroute changed unrelated 880 Hz route"
    );
    let after_move = capture_owned_endpoint(&capture.serial, input.directory, "rerouted");
    let unrelated_880 = tone_amplitude(&after_move, 880.0);
    assert!(unrelated_880 > 0.04, "external reroute muted unrelated app");
    update_private_target(input.selected.object_id, original_target.as_deref());
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let now = registry_snapshot();
        if default_metadata(&now) == defaults {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "private default metadata was not restored"
        );
        thread::sleep(Duration::from_millis(20));
    }
    input.backend.poll_source_mute(input.exchange);
    let after_restore = registry_snapshot();
    let ack_key = format!("ack.{}", input.selected.serial);
    assert!(
        metadata_value(
            &after_restore,
            "mixrelay-source-mute",
            render.object_id,
            &ack_key
        )
        .is_none(),
        "revoked lease was reinstalled without a new active plan"
    );
    let status = external.stop();
    assert!(status.success(), "external policy sink: {status}");
    serde_json::json!({
        "newSinkSerial": new_sink.serial,
        "movedLinks": moved_links,
        "unrelatedEightEightyHz": unrelated_880,
        "defaultMetadataRestored": true,
        "ackCleared": true,
        "externalSinkExit": status.success(),
    })
}

#[cfg(feature = "native-lab")]
fn collect_forward_graph_pcm(input: &mut ForwardInputs<'_, '_>) -> Value {
    let before = input.counters.transfer_observation();
    input.observer.send("phase 1");
    let armed = input.observer.event("armed", Some(1));
    let completed = input.observer.event("phaseComplete", Some(1));
    let after = input.counters.transfer_observation();
    let scheduling = continuity::scheduling_evidence(
        input
            .selected
            .process_id
            .expect("verified owned source retains its process ID"),
    );
    assert_eq!(completed["frames"].as_u64(), Some(WINDOW_FRAMES as u64));
    serde_json::json!({
        "boundaryContract": "control snapshot before phase 1 command and after phaseComplete; \
            includes command/event delivery delay, not exact PCM callback boundaries; \
            compare failures only for one unchanged nonzero stream owner; \
            first/last failure positions remain lifetime extrema, not interior zero-run positions",
        "before": before, "armed": armed, "completed": completed, "after": after,
        "schedulingBoundary": "control after phaseComplete and transfer snapshot, before source teardown; \
            process thread policies and current pressure, not a callback TID or event-time measurement",
        "scheduling": scheduling,
    })
}

#[cfg(feature = "native-lab")]
fn wait_driver_ready(
    driver: &mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'_>>,
    status: &mixrelay_engine_core::session::AdapterRuntimeStatus,
) {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    while !driver.ready() {
        assert!(
            !status.stop_requested(),
            "engine graph stopped before ready"
        );
        assert!(
            Instant::now() < deadline,
            "Linux GraphDriver did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native-lab")]
fn process_start_time(process_id: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{process_id}/stat"))
        .expect("read exact engine-process identity");
    stat.rsplit_once(')')
        .and_then(|(_, rest)| {
            rest.split_whitespace()
                .nth(19)
                .and_then(|value| value.parse::<u64>().ok())
        })
        .expect("parse exact engine-process start time")
}

#[cfg(feature = "native-lab")]
fn wait_for_owned_target(
    process_id: u32,
    app_id: &str,
    node_name: &str,
    media_class: &str,
) -> mixrelay_linux_audio_io::PipeWireTarget {
    let deadline = Instant::now() + SETUP_TIMEOUT;
    loop {
        let inventory = mixrelay_linux_audio_io::targets().expect("private PipeWire inventory");
        let candidates: Vec<_> = inventory
            .into_iter()
            .filter(|target| {
                target.process_id == Some(process_id)
                    && target.application_id.as_deref() == Some(app_id)
                    && target.node_name.as_deref() == Some(node_name)
                    && target.media_class == media_class
            })
            .collect();
        assert!(candidates.len() <= 1, "ambiguous test-owned node identity");
        if let Some(target) = candidates.into_iter().next() {
            assert!(!target.serial.is_empty(), "owned node lacks object serial");
            return target;
        }
        assert!(
            Instant::now() < deadline,
            "owned PipeWire node did not appear"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native-lab")]
fn verified_sink_node_name(ready: &Value, sink_process_id: u32) -> String {
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["routeMode"], "graph");
    assert_eq!(ready["applicationId"], SINK_APP_ID);
    assert_eq!(
        ready["processId"].as_u64(),
        Some(u64::from(sink_process_id))
    );
    let serial = ready["nodeSerial"]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("verified private Audio/Sink serial")
        .to_owned();
    let target = wait_for_owned_target(sink_process_id, SINK_APP_ID, SINK_NODE_NAME, "Audio/Sink");
    assert_eq!(ready["nodeId"].as_u64(), Some(u64::from(target.object_id)));
    assert_eq!(target.serial, serial);
    target
        .node_name
        .expect("verified sink has a persistent node name")
}

#[cfg(feature = "native-lab")]
fn verify_route_ready(route: &Value, process_id: u32, start_time: u64) {
    assert_eq!(route["event"], "routeReady");
    assert_eq!(route["processId"].as_u64(), Some(u64::from(process_id)));
    assert_eq!(route["processStartTime"].as_u64(), Some(start_time));
    assert!(route["sourceNodeSerial"].as_str().is_some());
    assert!(route["sinkNodeSerial"].as_str().is_some());
    let links = route["linkIds"]
        .as_array()
        .expect("exact graph route links");
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|id| id.as_u64().is_some_and(|id| id != 0)));
    assert_ne!(links[0], links[1]);
}

#[cfg(feature = "native-lab")]
fn stop_observer(observer: &mut super::mute::FixtureChild) -> ExitStatus {
    observer.send("quit");
    observer.wait()
}

#[cfg(feature = "native-lab")]
fn stop_recorder(recorder: &mut FixtureProcess) -> (bool, ExitStatus) {
    let running = recorder
        .0
        .try_wait()
        .expect("inspect independent source recorder before stop")
        .is_none();
    recorder.stop();
    let status = recorder.0.wait().expect("reap independent source recorder");
    (running, status)
}

#[cfg(feature = "native-lab")]
fn measure_tone(frames: &[[f32; 2]]) -> ToneContinuity {
    ToneContinuity::measure(frames.iter().map(|frame| frame[0]), SAMPLE_RATE, 1000.0)
        .expect("valid full-duration 1 kHz tone measurement")
}

#[cfg(feature = "native-lab")]
fn measure_sequence(frames: &[[f32; 2]]) -> SequenceContinuity {
    SequenceContinuity::measure(frames.iter().map(|frame| frame[1]))
}

#[cfg(feature = "native-lab")]
fn assert_tone_continuity(tone: &ToneContinuity, context: &str) {
    assert_eq!(tone.sample_count, WINDOW_FRAMES, "{context} tone duration");
    assert!(tone.intact(), "{context} tone gap: {tone:?}");
    assert!(
        (tone.peak - 0.125).abs() <= 0.001,
        "{context} tone level: {tone:?}"
    );
}

#[cfg(feature = "native-lab")]
fn assert_continuity(tone: &ToneContinuity, sequence: &SequenceContinuity, context: &str) {
    assert_tone_continuity(tone, context);
    assert_eq!(
        sequence.sample_count, WINDOW_FRAMES,
        "{context} sequence duration"
    );
    assert!(
        sequence.intact(),
        "{context} frame sequence gap: {sequence:?}"
    );
}

fn counter_delta(before: &Value, after: &Value) -> u64 {
    after
        .as_u64()
        .expect("unsigned after counter")
        .wrapping_sub(before.as_u64().expect("unsigned before counter"))
}

fn assert_clock_window(window: &Value, native_rate: u32) {
    let before = &window["before"];
    let after = &window["after"];
    for observation in [before, after] {
        assert_eq!(observation["available"], true, "coherent clock publication");
        assert_eq!(
            observation["device"]["streams"], 1,
            "one graph output owner"
        );
        assert_eq!(observation["engineClock"]["rate"], native_rate);
        let clock_id = observation["engineClock"]["id"].as_u64().unwrap();
        assert_ne!(clock_id, 0, "observed native graph clock");
        assert_eq!(
            observation["device"]["native_clock"],
            clock_id << 32 | u64::from(native_rate)
        );
    }
    assert_ne!(before["device"]["owner_sequence"], 0);
    assert_eq!(
        before["device"]["owner_sequence"],
        after["device"]["owner_sequence"]
    );
    assert_eq!(before["engineClock"]["id"], after["engineClock"]["id"]);
    assert_eq!(
        counter_delta(
            &before["engineClock"]["discontinuities"],
            &after["engineClock"]["discontinuities"]
        ),
        0,
        "native callback timeline changed within the duration window"
    );
    let initial = &before["device"]["counters"];
    let current = &after["device"]["counters"];
    for field in [
        "native_restart_calls",
        "consumer_discard_calls",
        "consumer_discard_frames",
    ] {
        assert_eq!(
            counter_delta(&initial[field], &current[field]),
            0,
            "{field}"
        );
    }
    for stage in 0..4 {
        for field in ["rejected_calls", "rejected_frames"] {
            assert_eq!(
                counter_delta(
                    &initial["failures"][stage][field],
                    &current["failures"][stage][field]
                ),
                0,
                "output stage {stage} {field} within duration window"
            );
        }
    }
    let callback_frames = counter_delta(
        &before["engineClock"]["callbackFrames"],
        &after["engineClock"]["callbackFrames"],
    );
    let written = counter_delta(&initial["written"], &current["written"]);
    assert!(callback_frames > 0 && written > 0, "no processed duration");
    // Output-ring written advances only from graph writes, never native reads.
    // Difference of two carried-remainder floors is strictly below one engine frame.
    let frame_error = (i128::from(written) * i128::from(native_rate)
        - i128::from(callback_frames) * i128::from(SAMPLE_RATE))
    .abs();
    assert!(
        frame_error < i128::from(native_rate),
        "native={callback_frames} written={written} rate={native_rate} frame error numerator={frame_error}"
    );
    println!(
        "native_duration native_rate={native_rate} callback_frames={callback_frames} \
         engine_rate={SAMPLE_RATE} written_frames={written} \
         frame_error_numerator={frame_error} frame_error_denominator={native_rate}"
    );
}

#[cfg(feature = "native-lab")]
fn load_graph_output(directory: &Path, process_id: u32) -> (Value, Vec<[f32; 2]>) {
    let metadata: Value = serde_json::from_slice(
        &std::fs::read(directory.join(format!("graph-sink-{process_id}.json")))
            .expect("graph-route observer metadata"),
    )
    .expect("valid graph-route observer metadata");
    assert_eq!(metadata["mode"], "graph-sink");
    assert_eq!(
        metadata["phase_frames"].as_u64(),
        Some(WINDOW_FRAMES as u64)
    );
    assert_eq!(metadata["frame_counts"], serde_json::json!([WINDOW_FRAMES]));
    assert_eq!(metadata["invalid_buffers"].as_u64(), Some(0));
    let raw_name = metadata["raw_file"]
        .as_str()
        .filter(|name| Path::new(name).file_name().and_then(|part| part.to_str()) == Some(*name))
        .expect("bounded graph observer raw filename");
    let raw = std::fs::read(directory.join(raw_name)).expect("graph output PCM raw trace");
    assert_eq!(raw.len(), WINDOW_FRAMES * 8);
    let frames = raw
        .chunks_exact(8)
        .map(|frame| {
            [
                f32::from_ne_bytes(frame[..4].try_into().unwrap()),
                f32::from_ne_bytes(frame[4..].try_into().unwrap()),
            ]
        })
        .collect();
    (metadata, frames)
}

#[test]
fn graph_document_is_exact_two_channel_capture_to_selected_sink() {
    let document = graph_document(1234, SINK_NODE_NAME);
    assert_eq!(document.nodes.len(), 2);
    assert_eq!(document.edges.len(), 1);
    assert_eq!(document.nodes[0].id.as_str(), "application");
    assert_eq!(document.nodes[1].id.as_str(), "engine-output");
    let NodeConfig::ApplicationCapture(capture) = &document.nodes[0].config else {
        panic!("first node must be the selected application capture");
    };
    assert_eq!(
        capture.selector,
        ApplicationSelector::ProcessId {
            process_id: 1234,
            include_descendants: false,
        }
    );
    assert_eq!(capture.source_playback, SourcePlaybackPolicy::Preserve);
    let NodeConfig::AudioDeviceOutput(output) = &document.nodes[1].config else {
        panic!("second node must be the selected AudioDeviceOutput");
    };
    assert!(matches!(
        &output.selector,
        DeviceSelector::PersistentUid { uid } if uid == SINK_NODE_NAME
    ));
    assert_eq!(output.input_channels, 2);
    assert_eq!(document.edges[0].source.node_id.as_str(), "application");
    assert_eq!(
        document.edges[0].destination.node_id.as_str(),
        "engine-output"
    );
    assert_eq!(
        ENGINE_OUTPUT_NODE_NAME,
        "io.mixrelay.engine-output.device-output"
    );
}

#[test]
fn composed_output_name_uses_the_canonical_native_output_id() {
    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    assert_eq!(composed.native_outputs.len(), 1);
    let native_output_key = format!("device:{}:{SINK_NODE_NAME}", SINK_NODE_NAME.len());
    assert_eq!(
        composed.native_outputs[&native_output_key].as_str(),
        "o:39:device:29:io.mixrelay.fixture.mute-sink:native"
    );
    assert_eq!(
        composed_output_node_name(&composed.native_outputs, SINK_NODE_NAME),
        "io.mixrelay.o_39_device_29_io.mixrelay.fixture.mute-sink_native.device-output"
    );
}

#[test]
fn composed_forward_intent_retains_exact_endpoint_without_authored_loop() {
    let authored = source_mute_graph(1234, SINK_NODE_NAME);
    assert_eq!(authored.edges.len(), 1);
    assert_eq!(authored.edges[0].id.as_str(), "application-to-sink");

    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    let endpoint_id = VirtualEndpointId::from(FORWARD_ENDPOINT_ID);
    assert!(composed.native_devices.contains_key(&endpoint_id));
    assert_eq!(composed.playback_intents.len(), 1);
    let intent = &composed.playback_intents[0];
    assert_eq!(intent.forwarding_endpoint, endpoint_id);
    assert!(
        composed
            .native_sources
            .values()
            .any(|id| id == &intent.capture_node_id)
    );
    assert!(
        composed
            .node_origins
            .get(&intent.mask_node_id)
            .is_some_and(|origin| {
                origin.graph_id == "source-mute" && origin.node_id.as_str() == "application"
            })
    );
}

#[test]
fn composed_forward_endpoint_uses_canonical_playback_without_fixture_feedback() {
    let composed = composed_source_mute_graph(1234, SINK_NODE_NAME);
    let endpoint_id = VirtualEndpointId::from(FORWARD_ENDPOINT_ID);
    let native = &composed.native_devices[&endpoint_id];
    let snapshot = &composed.device_buses[&endpoint_id];
    let render_edges = composed
        .document
        .edges
        .iter()
        .filter(|edge| {
            edge.source.node_id == *native && edge.source.port_id.as_str() == FROM_OS_PORT_ID
        })
        .collect::<Vec<_>>();
    assert_eq!(render_edges.len(), 1);
    // Native capture already contains the Driver's canonical sum. Reading it
    // feeds the snapshot directly; feeding it back to the render mixer would
    // duplicate that signal and create an unauthored feedback path.
    assert_eq!(&render_edges[0].destination.node_id, snapshot);
    assert_eq!(render_edges[0].destination.port_id.as_str(), AUDIO_PORT_ID);
    assert!(
        !composed.document.edges.iter().any(|edge| {
            edge.source.node_id == *snapshot && edge.destination.node_id == *native
        })
    );
    assert!(!composed.document.edges.iter().any(|edge| {
        let source = composed.node_origins.get(&edge.source.node_id);
        let destination = composed.node_origins.get(&edge.destination.node_id);
        source.is_some_and(|origin| {
            origin.node_id.as_str() == "owned-forward-endpoint"
                && origin
                    .meter_port
                    .as_ref()
                    .is_some_and(|port| port.as_str() == FROM_OS_PORT_ID)
        }) && destination.is_some_and(|origin| {
            origin.node_id.as_str() == "owned-forward-endpoint"
                && origin
                    .meter_port
                    .as_ref()
                    .is_some_and(|port| port.as_str() == TO_OS_PORT_ID)
        })
    }));
}
