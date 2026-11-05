use super::{
    preparation::{CapturePreparationError, observed_streams, prepare, wait_for_capture_format},
    selection::{matching_targets, serials},
    watch::WatchEntry,
    *,
};
#[cfg(feature = "native-lab")]
use crate::test_support::ObservedDriver;
use crate::{runtime::render_format, test_support::FixtureProcess};
use mixrelay_audio_graph::{ApplicationCaptureRequest, ApplicationSelector};
use mixrelay_linux_audio_io::{PipeWireTarget, process_identity::ProcessIdentity};
#[cfg(feature = "native-lab")]
use mixrelay_test_support::tone_continuity::ToneContinuity;

mod continuity;
mod graph_continuity;
mod mute;

fn target(serial: &str, pid: u32, app_id: &str) -> PipeWireTarget {
    PipeWireTarget {
        object_id: 1,
        serial: serial.into(),
        node_name: Some(format!("node-{serial}")),
        process_id: Some(pid),
        application_id: Some(app_id.into()),
        executable: Some("player".into()),
        icon_name: None,
        display_name: "Same display name".into(),
        media_class: "Stream/Output/Audio".into(),
        channels: 2,
    }
}
fn process(pid: u32, start: u64, parent: u32) -> ProcessIdentity {
    ProcessIdentity {
        process_id: pid,
        start_time: start,
        parent_id: parent,
        executable_path: Some("/usr/bin/player".into()),
    }
}
fn selector() -> ApplicationSelector {
    ApplicationSelector::BundleId {
        bundle_id: "org.test.player".into(),
        include_descendants: false,
    }
}
fn request(selector: &ApplicationSelector) -> ApplicationCaptureRequest {
    serde_json::from_value(serde_json::json!({"selector": selector, "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve"})).unwrap()
}
fn watch() -> WatchEntry {
    WatchEntry {
        owners: mixrelay_linux_pipewire::StreamOwners::default(),
        lease: CaptureLease(1),
        format: render_format(),
        node_id: "app".into(),
        request: request(&selector()),
        pinned_process: None,
        serials: Vec::new(),
        first_pcm: Vec::new(),
        healthy: Arc::new(AtomicBool::new(true)),
    }
}

#[test]
fn playback_target_requires_same_serial_and_first_pcm_from_live_process() {
    let fixture = FixtureProcess::start(std::path::Path::new("/bin/sleep"), &["30"]);
    let pid = fixture.id();
    let mut entry = watch();
    entry.serials = vec!["73".into()];
    let observed = Arc::new(AtomicBool::new(false));
    entry.first_pcm = vec![("73".into(), Arc::clone(&observed))];
    let watch = CaptureWatch {
        generation: 1,
        format: render_format(),
        entries: vec![entry],
    };
    let mut current = target("73", pid, "org.test.player");
    current.executable = Some("sleep".into());
    assert!(
        watch
            .verified_playback_targets("app", std::slice::from_ref(&current))
            .is_empty()
    );
    observed.store(true, Ordering::Release);
    let verified = watch.verified_playback_targets("app", std::slice::from_ref(&current));
    assert_eq!(verified.len(), 1);
    assert_eq!(verified[0].object_id, current.object_id);
    assert_eq!(verified[0].serial, "73");
    assert_eq!(verified[0].process.process_id, pid);
    assert!(
        watch
            .verified_playback_targets("app", &[target("74", pid, "org.test.player")])
            .is_empty()
    );
    assert!(
        watch
            .verified_playback_targets("app", &[target("73", u32::MAX, "org.test.player")])
            .is_empty()
    );
}

#[test]
fn exact_application_id_follows_new_streams_without_display_name_aliases() {
    let inventory = [
        target("a", 100, "org.test.player"),
        target("b", 101, "unrelated"),
    ];
    assert_eq!(
        serials(&matching_targets(&inventory, &selector(), None, |_| None)),
        ["a"]
    );
    let mut misleading = target("c", 102, "unrelated");
    misleading.executable = Some("org.test.player".into());
    assert!(matching_targets(&[misleading], &selector(), None, |_| None).is_empty());
    let restarted = [
        target("d", 200, "org.test.player"),
        target("e", 200, "org.test.player"),
    ];
    assert_eq!(
        serials(&matching_targets(&restarted, &selector(), None, |_| None)),
        ["d", "e"]
    );
}

#[test]
fn executable_selector_uses_kernel_path_not_basename_guess() {
    let selector = ApplicationSelector::ExecutablePath {
        path: "/usr/bin/player".into(),
    };
    let inventory = [target("a", 100, "ignored")];
    assert_eq!(
        matching_targets(&inventory, &selector, None, |pid| Some(process(pid, 1, 1))).len(),
        1
    );
    assert!(
        matching_targets(&inventory, &selector, None, |pid| Some(ProcessIdentity {
            executable_path: Some("/other/player".into()),
            ..process(pid, 1, 1)
        }))
        .is_empty()
    );
    let mut unrelated = inventory[0].clone();
    unrelated.executable = Some("other".into());
    assert!(
        matching_targets(&[unrelated], &selector, None, |pid| Some(process(
            pid, 1, 1
        )))
        .is_empty()
    );
}

#[test]
fn process_pin_rejects_exit_reuse_and_unpinned_adoption() {
    let selector = ApplicationSelector::ProcessId {
        process_id: 100,
        include_descendants: true,
    };
    let inventory = [target("a", 100, "app"), target("b", 101, "app")];
    let pin = process(100, 10, 1);
    let lookup = |pid| Some(process(pid, 10, if pid == 101 { 100 } else { 1 }));
    assert_eq!(
        matching_targets(&inventory, &selector, Some(&pin), lookup).len(),
        2
    );
    assert!(matching_targets(&inventory, &selector, None, lookup).is_empty());
    assert!(matching_targets(&inventory, &selector, Some(&pin), |_| None).is_empty());
    assert!(
        matching_targets(&inventory, &selector, Some(&pin), |pid| Some(process(
            pid, 99, 100
        )))
        .is_empty()
    );
    let reads = std::cell::Cell::new(0);
    assert!(
        matching_targets(&inventory[..1], &selector, Some(&pin), |pid| {
            reads.set(reads.get() + 1);
            Some(process(pid, if reads.get() == 1 { 10 } else { 99 }, 1))
        })
        .is_empty()
    );
}

#[test]
fn cancelled_capture_preparation_never_opens_or_publishes_native_owners() {
    let stopping = AtomicBool::new(true);
    assert!(matches!(
        prepare(
            &watch(),
            &[&target("native-must-not-open", 100, "app")],
            &stopping
        ),
        Err(CapturePreparationError::Cancelled)
    ));
    assert!(matches!(
        wait_for_capture_format(&[], &stopping),
        Err(CapturePreparationError::Cancelled)
    ));
    assert!(wait_for_capture_format(&[], &AtomicBool::new(false)).is_ok());
    let invalid = target("\0", 100, "app");
    let mut entry = watch();
    assert!(matches!(
        prepare(&entry, &[&invalid], &AtomicBool::new(false)),
        Err(CapturePreparationError::MissingPcmObservation)
    ));
    entry.first_pcm = observed_streams(&[&invalid]);
    assert!(matches!(
        prepare(&entry, &[&invalid], &AtomicBool::new(false)),
        Err(CapturePreparationError::Native(_))
    ));
}

#[test]
fn absent_new_removed_and_recreated_streams_drive_updates() {
    let mut entry = watch();
    assert!(entry.changed_targets(&[], |_| None).is_none());
    let active = [target("first", 100, "org.test.player")];
    assert_eq!(
        serials(&entry.changed_targets(&active, |_| None).unwrap()),
        ["first"]
    );
    entry.serials = vec!["first".into()];
    assert!(entry.changed_targets(&active, |_| None).is_none());
    assert!(entry.changed_targets(&[], |_| None).unwrap().is_empty());
    let replacement = [target("replacement", 200, "org.test.player")];
    assert_eq!(
        serials(&entry.changed_targets(&replacement, |_| None).unwrap()),
        ["replacement"]
    );
    entry.healthy.store(false, Ordering::Release);
    assert!(entry.changed_targets(&active, |_| None).is_some());
}

#[test]
fn absent_stable_app_prepares_silence_without_native_stream_or_server() {
    let (mut captures, watch) =
        CaptureSet::open_inventory(&[("app".into(), request(&selector()))], 5, &[]).unwrap();
    assert_eq!(captures.generation, 5);
    assert_eq!(watch.entries.len(), 1);
    let mut samples = [1.0; 8];
    let outcome = captures.read("app", &mut samples);
    assert_eq!(outcome.frames, 0);
    assert_eq!(outcome.activity, InputActivity::Unknown);
    assert_eq!(samples.map(f32::to_bits), [0; 8]);
}

#[test]
fn missing_capture_observation_fails_before_opening_any_native_stream() {
    let entry = watch();
    let target = target("unobserved", 100, "org.test.player");
    let result = prepare(&entry, &[&target], &AtomicBool::new(false));
    assert!(matches!(
        result,
        Err(CapturePreparationError::MissingPcmObservation)
    ));
}

#[test]
fn oversized_capture_read_is_silent_bounded_and_does_not_replace_storage() {
    let (mut captures, _) =
        CaptureSet::open_inventory(&[("app".into(), request(&selector()))], 5, &[]).unwrap();
    let capacity = captures.nodes["app"].scratch.len();
    let pointer = captures.nodes["app"].scratch.as_ptr();
    let mut samples = vec![1.0; capacity + 2];

    let outcome = captures.read("app", &mut samples);

    assert_eq!(outcome.frames, 0);
    assert_eq!(outcome.activity, InputActivity::Unknown);
    assert!(outcome.discontinuity);
    assert!(samples.iter().all(|sample| sample.to_bits() == 0));
    assert_eq!(captures.nodes["app"].scratch.len(), capacity);
    assert_eq!(captures.nodes["app"].scratch.as_ptr(), pointer);
    assert!(captures.nodes["app"].healthy.load(Ordering::Acquire));

    let valid = captures.read("app", &mut samples[..capacity]);
    assert_eq!(valid.frames, 0);
    assert_eq!(valid.activity, InputActivity::Unknown);
    assert!(!valid.discontinuity);
}

#[test]
fn stopped_or_empty_watch_never_inspects_registry() {
    let stopped = AtomicBool::new(true);
    let watcher = CaptureWatch {
        generation: 1,
        format: render_format(),
        entries: vec![watch()],
    };
    assert!(watcher.recover(&stopped).unwrap().is_none());
    let watcher = CaptureWatch {
        generation: 1,
        format: render_format(),
        entries: Vec::new(),
    };
    assert!(watcher.recover(&AtomicBool::new(false)).unwrap().is_none());
}

#[test]
fn restored_pid_requires_reselection_and_never_resolves_or_recaptures() {
    let selector = ApplicationSelector::ProcessId {
        process_id: std::process::id(),
        include_descendants: true,
    };
    let mut request = request(&selector);
    request.requires_reselection = true;
    let (mut capture, watch) = CaptureSet::open(&[("restored".into(), request)], 10).unwrap();
    assert!(matches!(
        capture.state(),
        mixrelay_engine_core::session::ApplicationRuntimeState::Waiting
    ));
    assert!(watch.recover(&AtomicBool::new(false)).unwrap().is_none());
    assert!(
        watch.entries[0]
            .changed_targets(&[target("a", std::process::id(), "old")], |_| panic!(
                "must not read PID"
            ))
            .is_none()
    );
    let mut samples = [1.0; 4];
    assert_eq!(capture.read("restored", &mut samples).frames, 0);
    assert_eq!(samples.map(f32::to_bits), [0; 4]);
}

#[cfg(feature = "native-lab")]
fn recover_until(
    captures: &mut CaptureSet,
    watch: &mut CaptureWatch,
    node_id: &str,
    present: bool,
) {
    let started = std::time::Instant::now();
    let mut replacements = 0;
    loop {
        if let Some((mut next, next_watch)) = watch.recover(&AtomicBool::new(false)).unwrap() {
            captures.replace(&mut next);
            *watch = next_watch;
            replacements += 1;
        }
        let mut samples = [0.0; 256];
        let outcome = captures.read(node_id, &mut samples);
        let frames = outcome.frames;
        let peak = samples.iter().copied().map(f32::abs).fold(0.0, f32::max);
        let node = captures
            .nodes
            .get(node_id)
            .expect("recovery must retain the requested capture node");
        let empty = node.taps.is_empty();
        if (present && frames > 0 && peak > 0.05)
            || (!present && empty && frames == 0 && peak == 0.0)
        {
            assert_eq!(
                outcome.activity,
                if present {
                    InputActivity::Active
                } else {
                    InputActivity::Unknown
                }
            );
            println!(
                "present={present}; native_frames={frames}; peak={peak}; taps={}; replacements={replacements}; elapsed_ms={}",
                node.taps.len(),
                started.elapsed().as_millis()
            );
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "native application recovery timed out: present={present}, frames={frames}, peak={peak}, empty={empty}, replacements={replacements}, formats={:?}, ready={:?}",
            node.taps
                .iter()
                .map(NativeStream::negotiated_format)
                .collect::<Vec<_>>(),
            node.taps
                .iter()
                .map(NativeStream::connected)
                .collect::<Vec<_>>()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_stream_relaunch_recovers_pcm_without_graph_reapply() {
    let _driver = ObservedDriver::new();
    let path = std::path::PathBuf::from(
        std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").expect("explicit fixture executable"),
    );
    assert!(path.is_absolute() && path.is_file());
    let selector = ApplicationSelector::BundleId {
        bundle_id: "org.mixrelay.fixture.application".into(),
        include_descendants: false,
    };
    let (mut captures, mut watch) =
        CaptureSet::open(&[("fixture".into(), request(&selector))], 9).unwrap();
    assert!(
        captures.nodes["fixture"].taps.is_empty(),
        "fixture must not already be running"
    );
    let first = FixtureProcess::start(&path, &[]);
    let first_pid = first.id();
    recover_until(&mut captures, &mut watch, "fixture", true);
    let apps = mixrelay_linux_audio_io::applications(&mixrelay_time::StdClock::new()).unwrap();
    assert_eq!(
        apps.iter()
            .find(|app| app.process_id == first_pid)
            .unwrap()
            .selector,
        selector
    );
    drop(first);
    recover_until(&mut captures, &mut watch, "fixture", false);
    let second = FixtureProcess::start(&path, &[]);
    assert_ne!(first_pid, second.id());
    recover_until(&mut captures, &mut watch, "fixture", true);
    let apps = mixrelay_linux_audio_io::applications(&mixrelay_time::StdClock::new()).unwrap();
    assert_eq!(
        apps.iter()
            .find(|app| app.process_id == second.id())
            .unwrap()
            .selector,
        selector
    );
    assert_eq!(captures.generation, 9);
    assert_eq!(captures.nodes["fixture"].taps.len(), 1);
    drop(second);
    recover_until(&mut captures, &mut watch, "fixture", false);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_pipewire_rate_conversion_and_observed_clock() {
    let driver = ObservedDriver::new();
    let path = std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let source = FixtureProcess::start(&path, &[]);
    let selector = ApplicationSelector::BundleId {
        bundle_id: "org.mixrelay.fixture.application".into(),
        include_descendants: false,
    };
    let (mut captures, mut watch) =
        CaptureSet::open(&[("fixture".into(), request(&selector))], 1).unwrap();
    recover_until(&mut captures, &mut watch, "fixture", true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut samples = Vec::with_capacity(53_000);
    let mut empty_reads = 0_u64;
    while samples.len() < 52_800 {
        let mut block = [0.0; 256];
        let frames = captures.read("fixture", &mut block).frames as usize;
        samples.extend(block[..frames * 2].chunks_exact(2).map(|frame| frame[0]));
        assert!(std::time::Instant::now() < deadline);
        if frames == 0 {
            empty_reads += 1;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    // Exclude startup converter history; count a full second of valid PCM.
    let samples = &samples[4800..52_800];
    let observed = driver.clock.load(Ordering::Acquire);
    let continuity = record_conversion_pcm(samples, empty_reads, captures.take_report(), observed);
    assert!(
        continuity.intact(),
        "converted PCM is discontinuous: {continuity:?}"
    );
    let crossings = samples
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();
    let frequency = f64::from(u32::try_from(crossings).unwrap()) * 48_000.0
        / f64::from(u32::try_from(samples.len()).unwrap());
    assert!(
        (frequency - 1000.0).abs() <= 3.0,
        "converted tone frequency={frequency}"
    );
    let rate = u32::try_from(observed & u64::from(u32::MAX)).unwrap();
    let id = u32::try_from(observed >> 32).unwrap();
    let expected: u32 = std::env::var("MIXRELAY_LAB_GRAPH_RATE")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(rate, expected);
    assert!(
        captures.shares_clock(id, rate),
        "stream and filter must observe the same actual driver"
    );
    println!(
        "graph_rate={rate}; source_rate={}; stream_pcm_rate=48000; measured_tone_hz={frequency}; samples={}; clock_id={id}; shared_driver=true",
        std::env::var("MIXRELAY_LAB_SOURCE_RATE").unwrap(),
        samples.len()
    );
    drop(captures);
    drop(source);
    drop(driver);
}

// Keep exact received PCM and native counters even when the frequency assertion
// fails. Evidence belongs to this test's explicitly owned private lab directory.
#[cfg(feature = "native-lab")]
fn record_conversion_pcm(
    samples: &[f32],
    empty_reads: u64,
    report: mixrelay_audio_io::process::ProcessReport,
    clock: u64,
) -> ToneContinuity {
    let fixture =
        std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let continuity = ToneContinuity::measure(samples.iter().copied(), 48_000, 1000.0).unwrap();
    let evidence = serde_json::json!({
        "streamPcmRate": 48000, "clockId": clock >> 32,
        "nativeGraphRate": clock & u64::from(u32::MAX),
        "emptyPollReads": empty_reads, "captureQueueOverflows": report.output_overflows,
        "counterScope": "capture producer queue; empty polls are not native device xruns",
        "continuity": continuity, "samples": samples,
    });
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(fixture.parent().unwrap().join("rate-conversion-pcm.json"))
        .unwrap();
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, &evidence).unwrap();
    std::io::Write::flush(&mut writer).unwrap();
    continuity
}
