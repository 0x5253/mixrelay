//! A serial, unchanged capture-cost probe. This is not an audio deadline test.
use mixrelay_audio_io::recording::{Encoding, RecorderAction, RecorderRequest};
use mixrelay_audio_recorder::RecorderRegistry;
use mixrelay_test_support::FixtureDir;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = FixtureDir::new("recorder-capture-cost")?;
    for trial in 0..3 {
        let registry = RecorderRegistry::threaded_files();
        let tap = registry.prepare("probe", 2)?;
        let samples = [0.25_f32; 512];
        tap.capture(&[], 48_000);
        registry.command(
            "probe",
            RecorderRequest {
                action: RecorderAction::Record,
                path: Some(
                    directory
                        .join(format!("trial-{trial}.wav"))
                        .to_string_lossy()
                        .into_owned(),
                ),
                sample_rate: Some(48_000),
                encoding: Some(Encoding::Float32),
            },
        )?;
        let mut timings = Vec::with_capacity(4096);
        for _ in 0..4096 {
            let start = Instant::now();
            tap.capture(&samples, 48_000);
            timings.push(start.elapsed().as_nanos());
            // Keep the writer serviceable without including sleep in the sample.
            std::thread::sleep(Duration::from_micros(300));
        }
        let status = registry.command(
            "probe",
            RecorderRequest {
                action: RecorderAction::Stop,
                path: None,
                sample_rate: None,
                encoding: None,
            },
        )?;
        assert_eq!(status.frames, 4096 * 256);
        timings.sort_unstable();
        println!(
            "trial={trial} frames={} calls={} mean_ns={} p50_ns={} p95_ns={} max_ns={}",
            status.frames,
            timings.len(),
            timings.iter().sum::<u128>() / u128::try_from(timings.len())?,
            timings[timings.len() / 2],
            timings[timings.len() * 95 / 100],
            timings[timings.len() - 1]
        );
    }
    Ok(())
}
