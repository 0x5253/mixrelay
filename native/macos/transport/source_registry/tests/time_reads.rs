//! Time-addressed reads report why they missed, and a pacing credit bounds the frontier.
use super::*;
use crate::application_source_transport::mr_application_source_transport_write_timed;
use crate::source_registry::content_latency::ReadTiming;

struct FixedClock(Instant);
impl MonotonicClock for FixedClock {
    fn now(&self) -> Instant {
        self.0
    }
}

const PERIOD: u64 = 1_000_000_000 / 48_000;

/// Acquisition time of `frame`; frames from 700 on follow a 1 ms coordinate gap.
fn time(frame: u64) -> u64 {
    let gap = if frame >= 700 { 1_000_000 } else { 0 };
    1_000_000_000 + frame * 1_000_000_000 / 48_000 + gap
}

/// Acknowledges the graph rate that the consumer's first read requested.
fn acknowledge(registry: &SourceRegistry, owner: &mut SourceTransport, token: u64) {
    let read = registry.read(token);
    let format = read
        .source()
        .unwrap()
        .mapping
        .clock()
        .format_word
        .load(Ordering::Acquire);
    // SAFETY: this fixture exclusively owns the producer.
    assert!(unsafe { mr_application_source_transport_reset(&raw mut *owner, format, 48_000) });
}

fn write_windows(owner: &mut SourceTransport, frames: std::ops::Range<u64>) {
    for first in frames.step_by(128) {
        let times: [u64; 128] = std::array::from_fn(|offset| time(first + offset as u64));
        // SAFETY: this fixture exclusively owns the producer; both spans contain 128 frames.
        unsafe {
            assert!(mr_application_source_transport_write_timed(
                &raw mut *owner,
                [0.5; 256].as_ptr(),
                times.as_ptr(),
                128
            ));
            mr_application_source_transport_publish_native(&raw const *owner, first + 128, true);
        }
    }
}

fn at(nanos: u64) -> ReadTiming {
    ReadTiming {
        rate: 48_000,
        first_nanos: Some(nanos),
        host_ticks: None,
    }
}

#[test]
fn time_addressed_misses_report_their_reason_without_advancing_the_cursor() {
    let registry = SourceRegistry::new(
        Box::new(FixedClock(Instant::from_origin(std::time::Duration::ZERO))),
        timebase(),
    );
    let mut owner = producer(&registry, &request("miss"));
    let absent = token(b"absent");
    let token = token(b"miss");
    let mut output = [0.0; 256];
    assert_eq!(
        registry.read_audio_timed(absent, &mut output, 128, at(time(0))),
        Err(ReadMiss::Unavailable)
    );
    // The first read requests the graph rate before the producer acknowledges it.
    assert_eq!(
        registry.read_audio_timed(token, &mut output, 128, at(time(0))),
        Err(ReadMiss::Unavailable)
    );
    acknowledge(&registry, &mut owner, token);
    assert_eq!(
        registry.read_audio_timed(token, &mut output, 128, at(time(0))),
        Err(ReadMiss::NotReady)
    );
    write_windows(&mut owner, 0..768);
    let measured = allocation_counter::measure(|| {
        assert_eq!(
            registry.read_audio_timed(token, &mut output, 128, at(time(767) + PERIOD)),
            Err(ReadMiss::Ahead)
        );
        assert_eq!(
            registry.read_audio_timed(token, &mut output, 128, at(time(512))),
            Ok(128)
        );
        // The forward-only cursor now starts at 640.
        assert_eq!(
            registry.read_audio_timed(token, &mut output, 128, at(time(0))),
            Err(ReadMiss::Behind)
        );
        assert_eq!(
            registry.read_audio_timed(token, &mut output, 128, at(time(640))),
            Err(ReadMiss::Mismatch)
        );
        assert_eq!(
            registry.read_audio_timed(token, &mut output, 128, at(time(767) + PERIOD)),
            Err(ReadMiss::Ahead)
        );
    });
    assert_eq!(measured.count_total, 0);
    assert!(output.iter().all(|sample| sample.to_bits() == 0));
}

#[test]
fn a_credited_publication_bounds_the_frontier_before_a_later_window() {
    let registry = SourceRegistry::new(
        Box::new(FixedClock(Instant::from_origin(std::time::Duration::ZERO))),
        timebase(),
    );
    let mut owner = producer(&registry, &request("credit"));
    let token = token(b"credit");
    let mut output = [0.0; 256];
    let _ = registry.read_audio_timed(token, &mut output, 128, at(time(0)));
    acknowledge(&registry, &mut owner, token);
    write_windows(&mut owner, 0..768);
    let progress = registry.pcm_progress(token).unwrap();
    let credited = PublishedCredit {
        epoch: progress.clock.epoch,
        through: progress.published_frames,
    };
    assert_eq!(credited.through, 768);
    // The producer commits another window before the graph requests its frontier.
    write_windows(&mut owner, 768..896);
    let frontier = |published| {
        registry.source_time_for_credit(
            token,
            48_000,
            GraphCredit {
                frames: 128,
                available_frames: 128,
                published,
            },
        )
    };
    let frame = |nanos: Option<u64>| (0..896).find(|frame| Some(time(*frame)) == nanos);
    let (bounded, newest) = (
        frame(frontier(Some(credited))).unwrap(),
        frame(frontier(None)).unwrap(),
    );
    // Both keep the same reserve; only the credited end differs.
    assert_eq!(newest - bounded, 128);
    assert!(bounded + 128 <= credited.through);
    let replaced = PublishedCredit {
        epoch: credited.epoch + (1 << 16),
        ..credited
    };
    assert_eq!(frontier(Some(replaced)), None);
}
