use super::*;
mod native_pcm;
use mixrelay_audio_io::{
    ChannelLayout, NativeSampleFormat, NativeStreamFormat, StreamSpec, VirtualEndpointSpec,
};

pub(super) fn config(channels: u16) -> DriverDeviceConfig {
    let stream = StreamSpec {
        channels,
        channel_layout: ChannelLayout::Discrete,
    };
    DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: VirtualEndpointId::new(format!("driver-fixture-{}-{channels}", std::process::id())),
            display_name: "MixRelay Driver fixture".into(),
            render: Some(stream),
            capture: Some(stream),
            supported_sample_rates: vec![48_000],
            default_sample_rate: 48_000,
        },
        render_format: None,
        capture_format: None,
    }
}

#[test]
fn native_configuration_rejects_wrong_direction_rate_and_geometry() {
    let mut requested = config(64);
    assert!(device::validate(&requested).is_ok());
    requested.capture_format = Some(NativeStreamFormat {
        sample_rate: 44_100,
        channels: 64,
        sample_format: NativeSampleFormat::Signed24,
    });
    assert!(device::validate(&requested).is_err());
    requested.capture_format.as_mut().unwrap().sample_rate = 48_000;
    assert!(device::validate(&requested).is_ok());
    requested.capture_format.as_mut().unwrap().channels = 32;
    assert!(device::validate(&requested).is_err());
    requested.capture_format.as_mut().unwrap().channels = 64;
    requested.spec.capture = None;
    assert!(device::validate(&requested).is_err());
}

#[test]
fn canonical_direction_names_preserve_identity_without_punctuation_aliases() {
    let mut device = config(32).spec;
    let first = native_names(&device);
    device.display_name = "Renamed".into();
    assert_eq!(first, native_names(&device));
    assert_ne!(
        native_name(&VirtualEndpointId::from("a/b")),
        native_name(&VirtualEndpointId::from("a_b"))
    );
    device.render = None;
    assert_eq!(native_names(&device), vec![first[1].clone()]);
}

#[test]
fn sixty_four_channel_projection_is_exact_and_allocation_free() {
    let input =
        std::array::from_fn::<_, 128, _>(|index| if index % 64 < 32 { 0.125 } else { -0.25 });
    let mut output = [0.0; 128];
    let measured = allocation_counter::measure(|| {
        for _ in 0..1000 {
            loopback::project(&input, &mut output, 64, 64);
        }
    });
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
    assert_eq!(measured.count_total, 0);
    let mut wider = [17.0; 128];
    loopback::project(&[0.5; 64], &mut wider, 32, 64);
    for frame in wider.chunks_exact(64) {
        assert!(
            frame[..32]
                .iter()
                .all(|sample| sample.to_bits() == 0.5_f32.to_bits())
        );
        assert!(frame[32..].iter().all(|sample| sample.to_bits() == 0));
    }
}

#[test]
#[ignore = "bounded release timing; run serially with no concurrent build or benchmark"]
fn capture_samples_release_timing() {
    use std::{hint::black_box, time::Instant};

    const ITERATIONS: u32 = 10_000;
    assert!(
        black_box(!cfg!(debug_assertions)),
        "run this measurement with --release"
    );
    for channels in [2, 32, 64] {
        for frames in [32, 128] {
            let input: Vec<f32> = (0..channels * frames)
                .map(|index| if index % 2 == 0 { 0.125 } else { -0.25 })
                .collect();
            let mut output = vec![17.0; input.len()];
            let allocations = allocation_counter::measure(|| {
                for _ in 0..ITERATIONS {
                    black_box(loopback::capture_samples(
                        black_box(&input),
                        black_box(&mut output),
                        black_box(channels),
                        black_box(channels),
                    ));
                }
            });
            assert_eq!(allocations.count_total, 0);
            let mut elapsed = [std::time::Duration::ZERO; 7];
            for run in 0..8 {
                let started = Instant::now();
                for _ in 0..ITERATIONS {
                    black_box(loopback::capture_samples(
                        black_box(&input),
                        black_box(&mut output),
                        black_box(channels),
                        black_box(channels),
                    ));
                }
                let duration = started.elapsed();
                let capture = loopback::capture_samples(&input, &mut output, channels, channels);
                assert_eq!(capture.len(), input.len());
                assert!(
                    capture
                        .iter()
                        .zip(&input)
                        .all(|(actual, expected)| { actual.to_bits() == expected.to_bits() })
                );
                if run != 0 {
                    elapsed[run - 1] = duration;
                }
            }
            elapsed.sort_unstable();
            eprintln!(
                "capture_samples channels={channels} frames={frames} calls={ITERATIONS} min_ns={:.3} median_ns={:.3} max_ns={:.3} allocations={}",
                elapsed[0].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
                elapsed[3].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
                elapsed[6].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
                allocations.count_total,
            );
        }
    }
}

#[test]
fn matching_width_capture_borrows_exact_pcm_without_output_storage() {
    for channels in [2, 32, 64] {
        for frames in [32, 128] {
            let input: Vec<f32> = (0..channels * frames)
                .map(|index| {
                    f32::from_bits([0x8000_0000, 0x7fc0_1234, 0x3e00_0000, 0xbe80_0000][index % 4])
                })
                .collect();
            let allocations = allocation_counter::measure(|| {
                let capture = loopback::capture_samples(&input, &mut [], channels, channels);
                assert_eq!(capture.as_ptr(), input.as_ptr());
                assert_eq!(capture.len(), input.len());
                assert!(
                    capture
                        .iter()
                        .zip(&input)
                        .all(|(actual, expected)| { actual.to_bits() == expected.to_bits() })
                );
            });
            assert_eq!(allocations.count_total, 0);
        }
    }
}

#[test]
fn unequal_width_capture_uses_output_and_silences_only_extra_lanes() {
    let input = [0.125, -0.25, 0.5, -0.75];
    let mut wider = [17.0; 8];
    let output_pointer = wider.as_ptr();
    let capture = loopback::capture_samples(&input, &mut wider, 2, 4);
    assert_eq!(capture.as_ptr(), output_pointer);
    assert_eq!(
        capture
            .iter()
            .copied()
            .map(f32::to_bits)
            .collect::<Vec<_>>(),
        [0.125, -0.25, 0.0, 0.0, 0.5, -0.75, 0.0, 0.0].map(f32::to_bits)
    );
    let mut narrower = [17.0; 2];
    let output_pointer = narrower.as_ptr();
    let capture = loopback::capture_samples(&input, &mut narrower, 4, 2);
    assert_eq!(capture.as_ptr(), output_pointer);
    assert_eq!(
        capture
            .iter()
            .copied()
            .map(f32::to_bits)
            .collect::<Vec<_>>(),
        [0.125, -0.25].map(f32::to_bits)
    );
}

#[test]
#[cfg(feature = "native-lab")]
fn native_driver_32_64_exact_formats_and_explicit_removal() {
    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let mut driver = LinuxDriver::open().unwrap();
    for channels in [32, 64] {
        let mut requested = config(channels);
        for sample_format in [
            NativeSampleFormat::Float32,
            NativeSampleFormat::Signed16,
            NativeSampleFormat::Signed24,
            NativeSampleFormat::Signed32,
        ] {
            let format = NativeStreamFormat {
                sample_rate: 48_000,
                channels,
                sample_format,
            };
            requested.render_format = Some(format);
            requested.capture_format = Some(format);
            driver.replace(&[requested.clone()]).unwrap();
            let settings = driver.settings().unwrap();
            assert_eq!(
                settings.devices[0].native_formats.render.current,
                Some(format)
            );
            assert_eq!(
                settings.devices[0].native_formats.capture.current,
                Some(format)
            );
            assert_eq!(driver.state(), DriverState::Ready);
        }
        assert_eq!(driver.settings().unwrap().devices.len(), 1);
    }
    driver.replace(&[]).unwrap();
    assert!(driver.settings().unwrap().devices.is_empty());
}
