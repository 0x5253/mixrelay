//! The source-mute forwarding scenario: lease, Off restore and external reroute.
use super::*;

pub(super) struct ForwardInputs<'a, 'b> {
    pub(super) driver: &'a mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'b>>,
    pub(super) status: &'a mixrelay_engine_core::session::AdapterRuntimeStatus,
    pub(super) counters: &'a DriverCounters,
    pub(super) backend: &'a crate::backend::LinuxVirtualEndpointBackend,
    pub(super) exchange: &'a Arc<mixrelay_render::ProcessorExchange>,
    pub(super) controls: &'a Arc<mixrelay_render::RuntimeControlBank>,
    pub(super) observer: &'a mut super::mute::FixtureChild,
    pub(super) directory: &'a Path,
    pub(super) selected: &'a mixrelay_linux_audio_io::PipeWireTarget,
    pub(super) endpoint_names: &'a [String],
    pub(super) intent: &'a mixrelay_audio_graph::composition::ApplicationPlaybackIntent,
    pub(super) engine_identity: EngineProcessIdentity,
}

pub(super) fn wait_for_playback_target(input: &ForwardInputs<'_, '_>) {
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

pub(super) fn wait_for_unrelated_output(
    process_id: u32,
) -> mixrelay_linux_audio_io::PipeWireTarget {
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

pub(super) fn publish_source_mask(input: &ForwardInputs<'_, '_>, revision: u64, enabled: bool) {
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

pub(super) fn run_forward_session(input: &mut ForwardInputs<'_, '_>) -> Value {
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

pub(super) fn driver_endpoint_targets(
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

pub(super) fn finish_forward_session(
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

pub(super) fn save_forward_transfer_observation(
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

pub(super) fn verify_off_restored(
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

pub(super) fn verify_external_reroute(
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

pub(super) fn collect_forward_graph_pcm(input: &mut ForwardInputs<'_, '_>) -> Value {
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
