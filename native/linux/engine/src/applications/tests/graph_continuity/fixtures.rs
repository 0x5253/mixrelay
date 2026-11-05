//! Owned helper processes: recorders, tone sources and endpoint captures.
use super::*;

pub(super) fn start_forward_source(
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

pub(super) fn capture_owned_endpoint(serial: &str, directory: &Path, label: &str) -> Vec<f32> {
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

pub(super) fn tone_amplitude(samples: &[f32], frequency: f64) -> f64 {
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

pub(super) fn unrelated_tone_file(directory: &Path) -> PathBuf {
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

pub(super) fn start_unrelated_tone(serial: &str, directory: &Path) -> FixtureProcess {
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

pub(super) fn stop_observer(observer: &mut super::mute::FixtureChild) -> ExitStatus {
    observer.send("quit");
    observer.wait()
}

pub(super) fn stop_recorder(recorder: &mut FixtureProcess) -> (bool, ExitStatus) {
    let running = recorder
        .0
        .try_wait()
        .expect("inspect independent source recorder before stop")
        .is_none();
    recorder.stop();
    let status = recorder.0.wait().expect("reap independent source recorder");
    (running, status)
}
