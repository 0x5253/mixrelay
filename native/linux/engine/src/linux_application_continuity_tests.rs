#[cfg(feature = "native-lab")]
use super::*;
#[cfg(feature = "native-lab")]
use mixrelay_test_support::frame_sequence::SequenceContinuity;
#[cfg(feature = "native-lab")]
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(feature = "native-lab")]
pub(super) fn independent_recorder(serial: &str, directory: &Path) -> FixtureProcess {
    // Probe only the CLI, outside audio timing. Help is not an audio client and
    // must not produce a fourth native-handoff journal when observation is active.
    let help = Command::new("pw-record")
        .arg("--help")
        .env_remove("LD_PRELOAD")
        .output()
        .unwrap();
    assert!(help.status.success(), "pw-record help probe failed");
    let raw = raw_capture_option(&String::from_utf8(help.stdout).unwrap());
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("independent-capture.f32"))
        .unwrap();
    let errors = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("independent-capture.stderr.log"))
        .unwrap();
    FixtureProcess(
        Command::new("pw-record")
            .args(raw)
            .args([
                "--rate=48000",
                "--channels=2",
                "--format=f32",
                "--latency=128",
                "--properties",
                concat!(
                    "node.name=io.mixrelay.lab-independent-capture node.dont-fallback=true ",
                    "node.dont-reconnect=true node.passive=true stream.dont-remix=true",
                ),
                "--target",
                serial,
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .spawn()
            .unwrap(),
    )
}

fn raw_capture_option(help: &str) -> Option<&'static str> {
    help.split_whitespace()
        .any(|word| word == "--raw")
        .then_some("--raw")
}

#[test]
fn independent_recorder_selects_the_advertised_raw_pcm_contract() {
    assert_eq!(
        raw_capture_option("  -a, --raw    RAW mode\n"),
        Some("--raw")
    );
    assert_eq!(raw_capture_option("  --format    Sample format\n"), None);
    assert_eq!(raw_capture_option("  --raw-format  Other option\n"), None);
}

#[cfg(feature = "native-lab")]
fn collect_native(captures: &mut CaptureSet) -> (Vec<[f32; 2]>, u64) {
    const FRAMES: usize = 48_000 * 10 + 4800;
    let mut samples = Vec::with_capacity(FRAMES + 128);
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut empty_polls = 0;
    while samples.len() < FRAMES {
        let mut block = [0.0; 256];
        let frames = captures.read("fixture", &mut block).frames as usize;
        samples.extend(
            block[..frames * 2]
                .chunks_exact(2)
                .map(|frame| [frame[0], frame[1]]),
        );
        assert!(Instant::now() < deadline, "native capture stalled");
        if frames == 0 {
            empty_polls += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    samples.truncate(FRAMES);
    (samples, empty_polls)
}

#[cfg(feature = "native-lab")]
pub(super) fn independent_continuity(directory: &Path) -> (ToneContinuity, SequenceContinuity) {
    let bytes = std::fs::read(directory.join("independent-capture.f32")).unwrap();
    assert_eq!(bytes.len() % 8, 0, "raw f32 stereo frame was truncated");
    let samples: Vec<f32> = bytes
        .chunks_exact(8)
        .map(|frame| f32::from_ne_bytes(frame[..4].try_into().unwrap()))
        .collect();
    assert!(
        samples.len() >= 48_000 * 9,
        "independent recorder received too little PCM"
    );
    // Separate clients start at different times; exclude converter startup/tail
    // separately rather than pretending their sample indices are aligned.
    let tone = ToneContinuity::measure(
        samples[4800..samples.len() - 4800].iter().copied(),
        48_000,
        1000.0,
    )
    .unwrap();
    let markers = bytes
        .chunks_exact(8)
        .skip(4800)
        .take(samples.len() - 9600)
        .map(|frame| f32::from_ne_bytes(frame[4..].try_into().unwrap()));
    (tone, SequenceContinuity::measure(markers))
}

#[cfg(feature = "native-lab")]
fn save_native_samples(directory: &Path, samples: &[[f32; 2]]) {
    let file = File::create_new(directory.join("native-capture.f32")).unwrap();
    let mut writer = BufWriter::new(file);
    for sample in samples.iter().flatten() {
        writer.write_all(&sample.to_ne_bytes()).unwrap();
    }
    writer.flush().unwrap();
}

#[cfg(feature = "native-lab")]
fn save_evidence(directory: &Path, evidence: &serde_json::Value) {
    let file = File::create_new(directory.join("independent-continuity.json")).unwrap();
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, evidence).unwrap();
    writer.flush().unwrap();
}

#[cfg(feature = "native-lab")]
fn assert_capture_completion(
    source: &serde_json::Value,
    recorder_running: bool,
    recorder_status: std::process::ExitStatus,
    recorder_errors: &str,
) {
    assert_eq!(source["valid"], true, "source trace: {source}");
    // PipeWire 0.3.65 pw-record returns 1 on SIGTERM; capture has no playback
    // drain-success event. Reject early exits, forced kills and stderr errors.
    assert!(recorder_running && matches!(recorder_status.code(), Some(0 | 1)));
    assert!(
        recorder_errors.is_empty(),
        "independent recorder: {recorder_errors}"
    );
}

#[cfg(feature = "native-lab")]
fn thread_policies(pid: u32) -> Option<String> {
    Command::new("ps")
        .args([
            "-L",
            "-p",
            &pid.to_string(),
            "-o",
            "pid,tid,cls,rtprio,comm",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(feature = "native-lab")]
pub(super) fn scheduling_evidence(source_pid: u32) -> serde_json::Value {
    // Optional cgroup v2 records are measurements, not a host-specific test gate.
    // Absence is retained explicitly rather than reported as zero pressure.
    serde_json::json!({
        "cpuStat": std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok(),
        "cpuPressure": std::fs::read_to_string("/sys/fs/cgroup/cpu.pressure").ok(),
        "memoryEvents": std::fs::read_to_string("/sys/fs/cgroup/memory.events").ok(),
        "guestCpuPressure": std::fs::read_to_string("/proc/pressure/cpu").ok(),
        "nativeThreadPolicies": thread_policies(std::process::id()),
        "sourceThreadPolicies": thread_policies(source_pid),
    })
}

#[cfg(feature = "native-lab")]
pub(super) fn finish_source(source: &mut FixtureProcess, directory: &Path) -> serde_json::Value {
    let name = format!("source-{}.json", source.0.id());
    let running = source.0.try_wait().unwrap().is_none();
    source.stop();
    let status = source.0.wait().unwrap();
    let trace = std::fs::read(directory.join(&name))
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| error.to_string())
        });
    let (trace, error) = match trace {
        Ok(trace) => (trace, None),
        Err(error) => (serde_json::Value::Null, Some(error)),
    };
    let callbacks = trace["events"].as_array().map(Vec::len);
    let expected_rate: u32 = std::env::var("MIXRELAY_LAB_SOURCE_RATE")
        .unwrap_or_else(|_| "48000".into())
        .parse()
        .unwrap();
    let dispatch = std::env::var("MIXRELAY_LAB_SOURCE_DISPATCH").unwrap_or_else(|_| "main".into());
    let expected_dispatch = match dispatch.as_str() {
        "main" => "main-loop, MAP_BUFFERS only",
        "data" => "data-loop, MAP_BUFFERS | RT_PROCESS",
        value => panic!("invalid source dispatch case {value}"),
    };
    let valid = running
        && status.success()
        && callbacks.is_some_and(|count| count > 0)
        && trace["dropped_records"] == 0
        && trace["channels"] == 2
        && trace["right_channel"] == "frame-sequence-f32-23bit"
        && trace["callback_dispatch"] == expected_dispatch
        && trace["sample_rate"] == expected_rate;
    // Preserve PCM even when source finalization/trace fails. Assert only after
    // both native recording and this structured failure have reached disk.
    serde_json::json!({"traceFile": name, "callbacks": callbacks,
        "exitCode": status.code(), "status": status.to_string(),
        "runningBeforeStop": running, "error": error, "valid": valid,
        "dispatch": trace["callback_dispatch"], "sampleRate": trace["sample_rate"]})
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_continuity_has_independent_capture_evidence() {
    let driver = ObservedDriver::new();
    let path = std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let mut source = FixtureProcess::start(&path, &[]);
    let selector = ApplicationSelector::BundleId {
        bundle_id: "org.mixrelay.fixture.application".into(),
        include_descendants: false,
    };
    let (mut captures, mut watch) =
        CaptureSet::open(&[("fixture".into(), request(&selector))], 1).unwrap();
    recover_until(&mut captures, &mut watch, "fixture", true);
    assert_eq!(watch.entries.len(), 1);
    assert_eq!(watch.entries[0].serials.len(), 1);
    let directory = path.parent().unwrap();
    let mut recorder = independent_recorder(&watch.entries[0].serials[0], directory);
    let scheduling_before = scheduling_evidence(source.0.id());
    let (samples, empty_polls) = collect_native(&mut captures);
    let scheduling_after = scheduling_evidence(source.0.id());
    let recorder_running = recorder.0.try_wait().unwrap().is_none();
    recorder.stop();
    let recorder_status = recorder.0.wait().unwrap();
    let recorder_errors =
        std::fs::read_to_string(directory.join("independent-capture.stderr.log")).unwrap();
    let source_evidence = finish_source(&mut source, directory);
    save_native_samples(directory, &samples);
    let left: Vec<f32> = samples[4800..].iter().map(|frame| frame[0]).collect();
    let native = record_conversion_pcm(
        &left,
        empty_polls,
        captures.take_report(),
        driver.clock.load(Ordering::Acquire),
    );
    let (independent, independent_sequence) = independent_continuity(directory);
    let native_sequence = SequenceContinuity::measure(samples[4800..].iter().map(|frame| frame[1]));
    let sequence_applicable = source_evidence["sampleRate"] == 48_000
        && (driver.clock.load(Ordering::Acquire) & u64::from(u32::MAX)) == 48_000;
    let evidence = serde_json::json!({
        "native": native, "independent": independent,
        "source": source_evidence,
        "frameSequence": { "applicable": sequence_applicable,
            "scope": "right channel, bit-preserving f32 at 48000 Hz; not valid across SRC",
            "native": native_sequence, "independent": independent_sequence },
        "schedulingBefore": scheduling_before, "schedulingAfter": scheduling_after,
        "independentTermination": {
            "runningBeforeStop": recorder_running, "exitCode": recorder_status.code(),
            "stderr": recorder_errors,
        },
    });
    save_evidence(directory, &evidence);
    println!("independent_capture_continuity={evidence}");
    assert_capture_completion(
        &source_evidence,
        recorder_running,
        recorder_status,
        &recorder_errors,
    );
    assert!(
        native.intact(),
        "native capture is discontinuous: {native:?}"
    );
    assert!(
        independent.intact(),
        "independent capture is discontinuous: {independent:?}"
    );
    if sequence_applicable {
        assert!(
            native_sequence.intact(),
            "native frame sequence: {native_sequence:?}"
        );
        assert!(
            independent_sequence.intact(),
            "independent frame sequence: {independent_sequence:?}"
        );
    }
}
