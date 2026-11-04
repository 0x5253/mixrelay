//! The measured age follows the PCM cursor, including partial and untimed reads.
use super::*;
use crate::application_source_transport::mr_application_source_transport_write_timed;
use crate::source_registry::content_latency::ReadTiming;

struct FixedClock(Instant);
impl MonotonicClock for FixedClock {
    fn now(&self) -> Instant {
        self.0
    }
}

#[test]
fn application_age_measures_consumed_coordinates_and_unknown_never_borrows_newer_time() {
    for timestamp in [Some(1_000_000_000_u64), None, Some(3_000_000_000)] {
        let registry = SourceRegistry::with_storage(
            Box::new(FixedClock(Instant::from_origin(std::time::Duration::ZERO))),
            timebase(),
            storage,
        );
        let mut owner = producer(&registry, &request("age"));
        let token = token(b"age");
        let mut output = [0.0; 256];
        registry.read_audio(token, &mut output, 128, 48_000);
        let before = registry.content_latency(token).unwrap().age.unwrap();
        let source = registry.read(token);
        let format = source
            .source()
            .unwrap()
            .mapping
            .clock()
            .format_word
            .load(Ordering::Acquire);
        let times: [u64; 768] = std::array::from_fn(|frame| {
            timestamp.map_or(0, |start| start + frame as u64 * 1_000_000_000 / 48_000)
        });
        // SAFETY: this fixture owns the producer and complete PCM/coordinate spans.
        unsafe {
            assert!(mr_application_source_transport_reset(
                &raw mut owner,
                format,
                48_000
            ));
            assert!(mr_application_source_transport_write_timed(
                &raw mut owner,
                [0.25; 1536].as_ptr(),
                times.as_ptr(),
                768
            ));
            mr_application_source_transport_publish_native(&raw const owner, 768, true);
        }
        let label = 2_000_000_000;
        let timing = ReadTiming {
            rate: 48_000,
            first_nanos: None,
            host_ticks: Some(
                registry
                    .timebase
                    .ticks(std::time::Duration::from_nanos(label)),
            ),
        };
        let allocations = allocation_counter::measure(|| {
            assert_eq!(
                registry.read_audio_time(token, &mut output, 128, timing),
                128
            );
            assert_eq!(
                registry.read_audio_time(token, &mut output, 128, timing),
                128
            );
        });
        assert_eq!(allocations.count_total, 0);
        assert_eq!(output.map(f32::to_bits), [0.25_f32; 256].map(f32::to_bits));
        let after = registry.content_latency(token).unwrap().age.unwrap();
        if timestamp == Some(1_000_000_000) {
            // The policy starts at 768 - 512 = 256. Two partial reads consume
            // frames 256..512, not the most recently published callback's time.
            let expected = (256..512).map(|frame| label - times[frame]).sum::<u64>() / 256;
            assert!(
                (after.mean_since(before).unwrap() - f64::from(u32::try_from(expected).unwrap()))
                    .abs()
                    <= 1.0
            );
        } else {
            assert!(after.invalid > before.invalid);
            assert_eq!(after.mean_since(before), None);
        }
    }
}

fn storage(bytes: usize) -> std::io::Result<std::os::fd::OwnedFd> {
    let directory = mixrelay_test_support::FixtureDir::new("app-age")?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.path().join("pcm"))?;
    file.set_len(u64::try_from(bytes).unwrap())?;
    Ok(file.into())
}
