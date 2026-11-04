//! Independent PCM capture and continuity checks saved as lab evidence.
use super::*;

pub(super) struct GraphCaptureEvidence {
    pub(super) directory: PathBuf,
    pub(super) frames: Vec<[f32; 2]>,
    pub(super) transfer_before: Option<mixrelay_linux_pipewire::TransferSnapshot>,
    pub(super) transfer_after: Option<mixrelay_linux_pipewire::TransferSnapshot>,
    pub(super) source_sample_faults: u64,
    pub(super) saved: bool,
}

impl GraphCaptureEvidence {
    pub(super) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            frames: Vec::with_capacity(WINDOW_FRAMES),
            transfer_before: None,
            transfer_after: None,
            source_sample_faults: 0,
            saved: false,
        }
    }

    pub(super) fn collect(&mut self, captures: &mut CaptureSet) {
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

    pub(super) fn assert_no_native_capture_loss(&self) {
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

    pub(super) fn save(&mut self, evidence: &Value) -> std::io::Result<()> {
        self.saved = true;
        self.write_files(evidence)
    }

    pub(super) fn write_files(&self, evidence: &Value) -> std::io::Result<()> {
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

pub(super) struct GraphValidation {
    pub(super) directory: PathBuf,
    pub(super) result: GraphWindowResult,
    pub(super) observer_id: u32,
    pub(super) recorder_running: bool,
    pub(super) recorder_status: ExitStatus,
    pub(super) source_summary: Value,
    pub(super) independent_tone: ToneContinuity,
    pub(super) independent_sequence: SequenceContinuity,
}

pub(super) struct GraphPcmResults {
    pub(super) sink_metadata: Value,
    pub(super) input_tone: ToneContinuity,
    pub(super) input_sequence: SequenceContinuity,
    pub(super) output_tone: ToneContinuity,
    pub(super) output_sequence: SequenceContinuity,
}

pub(super) fn validate_graph_evidence(evidence: &mut GraphCaptureEvidence, run: &GraphValidation) {
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

pub(super) fn assert_graph_pcm(pcm: &GraphPcmResults, run: &GraphValidation) {
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

pub(super) fn save_graph_summary(
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

pub(super) struct GraphWindowResult {
    pub(super) route: Value,
    pub(super) observer_status: ExitStatus,
    pub(super) stats: DriverStats,
    pub(super) engine_stopped: bool,
    pub(super) output_window: Value,
}

#[derive(Clone, Copy)]
pub(super) struct EngineProcessIdentity {
    pub(super) process_id: u32,
    pub(super) start_time: u64,
}

pub(super) fn capture_graph_window(
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

pub(super) fn measure_tone(frames: &[[f32; 2]]) -> ToneContinuity {
    ToneContinuity::measure(frames.iter().map(|frame| frame[0]), SAMPLE_RATE, 1000.0)
        .expect("valid full-duration 1 kHz tone measurement")
}

pub(super) fn measure_sequence(frames: &[[f32; 2]]) -> SequenceContinuity {
    SequenceContinuity::measure(frames.iter().map(|frame| frame[1]))
}

pub(super) fn assert_tone_continuity(tone: &ToneContinuity, context: &str) {
    assert_eq!(tone.sample_count, WINDOW_FRAMES, "{context} tone duration");
    assert!(tone.intact(), "{context} tone gap: {tone:?}");
    assert!(
        (tone.peak - 0.125).abs() <= 0.001,
        "{context} tone level: {tone:?}"
    );
}

pub(super) fn assert_continuity(
    tone: &ToneContinuity,
    sequence: &SequenceContinuity,
    context: &str,
) {
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

pub(super) fn load_graph_output(directory: &Path, process_id: u32) -> (Value, Vec<[f32; 2]>) {
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
