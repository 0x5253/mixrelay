//! Paced 32-frame debug-profile recorder reproduction, not a native clock proof.
use mixrelay_audio_io::recording::{Encoding, RecorderAction, RecorderRequest, RecorderState};
use mixrelay_audio_recorder::{RecorderRegistry, RecorderTap};
use mixrelay_test_support::FixtureDir;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output_rate = std::env::args()
        .nth(1)
        .map_or(Ok(96_000), |value| value.parse::<u32>())?;
    let encoding = match std::env::args().nth(2).as_deref() {
        None | Some("pcm24") => Encoding::Pcm24,
        Some("float32") => Encoding::Float32,
        Some(_) => return Err("encoding must be pcm24 or float32".into()),
    };
    let directory = FixtureDir::new("recorder-paced")?;
    let channels = std::env::args()
        .nth(3)
        .map_or(Ok(2), |value| value.parse::<u16>())?;
    let registry = RecorderRegistry::threaded_files();
    let tap = registry.prepare("paced", channels)?;
    let samples = vec![0.25; usize::from(channels) * 32];
    tap.capture(&[], 48_000);
    let preparation = Instant::now();
    registry.command(
        "paced",
        RecorderRequest {
            action: RecorderAction::Record,
            path: Some(directory.join("paced.wav").to_string_lossy().into_owned()),
            sample_rate: Some(output_rate),
            encoding: Some(encoding),
        },
    )?;
    println!(
        "record_preparation_us={}",
        preparation.elapsed().as_micros()
    );
    let elapsed = capture_take(&registry, &tap, &samples, output_rate)?;
    let status = registry.command(
        "paced",
        RecorderRequest {
            action: RecorderAction::Stop,
            path: None,
            sample_rate: None,
            encoding: None,
        },
    )?;
    assert_eq!(status.frames, u64::from(output_rate) * 10);
    println!(
        "finalized_frames={} capture_elapsed_ms={}",
        status.frames,
        elapsed.as_millis()
    );
    Ok(())
}

fn capture_take(
    registry: &RecorderRegistry,
    tap: &RecorderTap,
    samples: &[f32],
    output_rate: u32,
) -> Result<Duration, Box<dyn std::error::Error>> {
    let start = Instant::now();
    let mut late = Duration::ZERO;
    let mut max_backlog = 0;
    for index in 0..15_000_u32 {
        let deadline = start + Duration::from_nanos(u64::from(index) * 1_000_000_000 / 1500);
        if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
        late = late.max(Instant::now().saturating_duration_since(deadline));
        tap.capture(samples, 48_000);
        if index % 150 == 149 {
            let status = registry.status("paced");
            let expected = u64::from(index + 1) * 32 * u64::from(output_rate) / 48_000;
            max_backlog = max_backlog.max(expected.saturating_sub(status.frames));
            if index % 1500 == 1499 || status.state == RecorderState::Failed {
                println!(
                    "calls={} elapsed_ms={} written={} backlog_max={} late_max_us={} state={:?}",
                    index + 1,
                    start.elapsed().as_millis(),
                    status.frames,
                    max_backlog,
                    late.as_micros(),
                    status.state
                );
            }
            if status.state == RecorderState::Failed {
                return Err(status.error.unwrap_or_default().into());
            }
        }
    }
    Ok(start.elapsed())
}
