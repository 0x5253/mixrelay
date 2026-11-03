use mixrelay_audio_io::pcm_fifo::pcm_fifo;
use std::{hint::black_box, time::Instant};

#[test]
#[ignore = "serial release PCM throughput measurement, no native devices"]
fn measure_pcm_roundtrip() {
    const FRAMES: usize = 128;
    const ITERATIONS: usize = 20_000;
    for channels in [1, 32, 64] {
        let (producer, consumer) = pcm_fifo(channels, 4096).unwrap();
        let source = vec![0.25; channels * FRAMES];
        let mut destination = vec![0.0; source.len()];
        for _ in 0..1000 {
            assert_eq!(producer.write(black_box(&source)), FRAMES);
            assert_eq!(consumer.read(black_box(&mut destination)), FRAMES);
        }
        let allocations = allocation_counter::measure(|| {
            for _ in 0..1000 {
                assert_eq!(producer.write(black_box(&source)), FRAMES);
                assert_eq!(consumer.read(black_box(&mut destination)), FRAMES);
            }
        });
        assert_eq!(allocations.count_total, 0);
        for run in 1..=5 {
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                assert_eq!(producer.write(black_box(&source)), FRAMES);
                assert_eq!(consumer.read(black_box(&mut destination)), FRAMES);
            }
            let elapsed = start.elapsed();
            assert_eq!(destination, source);
            eprintln!(
                "{}",
                serde_json::json!({
                    "channels": channels, "frames": FRAMES, "capacityFrames": 4096,
                    "iterations": ITERATIONS, "run": run, "elapsedNs": elapsed.as_nanos(),
                    "allocations": allocations.count_total,
                })
            );
        }
    }
}
