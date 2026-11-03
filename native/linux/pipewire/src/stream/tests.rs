use super::*;

#[test]
fn native_format_readback_decodes_all_encodings_from_one_tuple() {
    use mixrelay_audio_io::NativeSampleFormat;
    for (code, sample_format) in [
        (1, NativeSampleFormat::Float32),
        (2, NativeSampleFormat::Signed16),
        (3, NativeSampleFormat::Signed24),
        (4, NativeSampleFormat::Signed32),
    ] {
        let tuple = (96_000_u64 << 32) | (code << 16) | 64;
        let actual = decode_native_format(tuple).unwrap();
        assert_eq!(actual.sample_rate, 96_000);
        assert_eq!(actual.channels, 64);
        assert_eq!(actual.sample_format, sample_format);
    }
    assert_eq!(decode_native_format(0), None);
    assert_eq!(
        decode_native_format((48_000_u64 << 32) | (5 << 16) | 2),
        None
    );
}

#[test]
fn output_roles_share_pcm_mode_but_select_their_actual_publisher_timing() {
    let manual = StreamRole::DeviceOutput("pinned-sink");
    let automatic = StreamRole::AutomaticDeviceOutput("pinned-sink");
    assert_eq!(manual.native(), automatic.native());
    assert!(manual.native_output() && automatic.native_output());
    assert!(manual.manual_output());
    assert!(!automatic.manual_output());
    assert!(StreamRole::VirtualCapture.manual_output());
    let contribution = StreamRole::GraphContribution("driver-sink");
    assert_eq!(contribution.native(), (5, Some("driver-sink")));
    assert!(contribution.native_output() && contribution.manual_output());
    assert!(!StreamRole::DeviceInput("pinned-source").manual_output());
}

#[test]
#[cfg(feature = "native-lab")]
fn native_property_transaction_pins_stream_until_final_control_release() {
    use std::time::{Duration, Instant};
    let owners = StreamOwners::default();

    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    assert_eq!(owners.active_owners(), 0, "run native tests serially");
    for (role, suffix) in [
        (StreamRole::VirtualRender, "render"),
        (StreamRole::VirtualCapture, "capture"),
    ] {
        let name = format!(
            "io.mixrelay.lab-property-pin-{}-{suffix}",
            std::process::id()
        );
        let stream = NativeStream::open_format(
            &owners,
            &name,
            "Pinned stream",
            role,
            64,
            StreamFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
        )
        .unwrap();
        let properties = stream.properties();
        let weak_copy = properties.clone();
        let deadline = Instant::now() + Duration::from_secs(3);
        let node_id = loop {
            if let Some(id) = properties.node_id() {
                break id;
            }
            assert!(Instant::now() < deadline, "native node was not published");
            std::thread::sleep(Duration::from_millis(5));
        };
        let change = properties
            .description_change("Pinned stream", "Renamed stream")
            .unwrap();
        drop(stream);
        assert_eq!(owners.active_owners(), 1);
        assert_eq!(weak_copy.node_id(), Some(node_id));
        change.apply().unwrap();
        change.rollback().unwrap();
        drop(change);
        assert_eq!(owners.active_owners(), 0);
        assert_eq!(properties.node_id(), None);
        assert_eq!(weak_copy.node_id(), None);
        assert!(weak_copy.description_change("old", "new").is_err());

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let dump = std::process::Command::new("pw-dump").output().unwrap();
            assert!(dump.status.success());
            let nodes: serde_json::Value = serde_json::from_slice(&dump.stdout).unwrap();
            let present = nodes.as_array().unwrap().iter().any(|node| {
                node.pointer("/info/props/node.name")
                    .and_then(serde_json::Value::as_str)
                    == Some(name.as_str())
            });
            if !present {
                break;
            }
            assert!(Instant::now() < deadline, "retired native node leaked");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn invalid_identity_fails_before_opening_a_native_stream() {
    let format = StreamFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    for (name, description, role) in [
        ("bad\0name", "description", StreamRole::VirtualRender),
        ("name", "bad\0description", StreamRole::VirtualCapture),
        (
            "name",
            "description",
            StreamRole::DeviceInput("bad\0serial"),
        ),
    ] {
        assert!(
            NativeStream::open_format(&StreamOwners::default(), name, description, role, 2, format)
                .is_err()
        );
    }
}

#[test]
fn invalid_channel_width_fails_before_native_context_creation() {
    for channels in [0, 65, u16::MAX] {
        let result = NativeStream::open_format(
            &StreamOwners::default(),
            "name",
            "description",
            StreamRole::VirtualRender,
            channels,
            StreamFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
        );
        assert_eq!(
            result.err().as_deref(),
            Some("invalid PipeWire channel count")
        );
    }
}

#[test]
fn delayed_capture_retains_four_seconds_at_the_highest_product_rate() {
    use mixrelay_pcm_ring::{PcmRing, mr_pcm_ring_read_available, mr_pcm_ring_write};
    let roles = [
        StreamRole::VirtualRender,
        StreamRole::ApplicationCapture("source-serial"),
        StreamRole::DeviceInput("device-serial"),
    ];
    for role in roles {
        let mut ring = PcmRing::new(2, role.retention_frames()).unwrap();
        let input: Vec<f32> = (0_u32..192_000 * 4)
            .flat_map(|frame| {
                let sample = if (frame / 192_000).is_multiple_of(2) {
                    0.0
                } else {
                    f32::from_bits(0x3f00_0000 + frame)
                };
                [sample, -sample]
            })
            .collect();
        let mut result = vec![0.0; input.len()];
        let measurement = allocation_counter::measure(|| {
            for block in input.chunks_exact(256) {
                // SAFETY: this fixture owns the sole producer and complete PCM block.
                assert_eq!(
                    unsafe { mr_pcm_ring_write(&raw mut ring, block.as_ptr(), 128) },
                    128
                );
            }
            assert_eq!(ring.stats().overflows, 0);
            for block in result.chunks_exact_mut(256) {
                // SAFETY: writes are finished; this is the sole consumer with complete output storage.
                assert_eq!(
                    unsafe { mr_pcm_ring_read_available(&raw mut ring, block.as_mut_ptr(), 128) },
                    128
                );
            }
        });
        assert_eq!(measurement.count_total, 0);
        assert!(
            result
                .iter()
                .zip(&input)
                .all(|(actual, expected)| actual.to_bits() == expected.to_bits())
        );
        assert_eq!(ring.stats().queued, 0);
    }
}

#[test]
fn invalid_geometry_and_empty_target_fail_before_native_connection() {
    for (sample_rate, block_frames) in [(0, 128), (384_001, 128), (48_000, 0), (48_000, 8193)] {
        assert_eq!(
            NativeStream::open_format(
                &StreamOwners::default(),
                "name",
                "description",
                StreamRole::VirtualRender,
                2,
                StreamFormat {
                    sample_rate,
                    block_frames
                },
            )
            .err()
            .as_deref(),
            Some("invalid PipeWire stream format"),
        );
    }
    for role in [
        StreamRole::ApplicationCapture(""),
        StreamRole::DeviceInput(""),
        StreamRole::DeviceOutput(""),
    ] {
        assert_eq!(
            NativeStream::open_format(
                &StreamOwners::default(),
                "name",
                "description",
                role,
                2,
                StreamFormat {
                    sample_rate: 48_000,
                    block_frames: 128
                },
            )
            .err()
            .as_deref(),
            Some("invalid PipeWire target"),
        );
    }
}

#[test]
fn maximum_native_output_cycle_admits_and_primes_complete_pcm() {
    use mixrelay_pcm_ring::{PcmRing, mr_pcm_ring_read, mr_pcm_ring_write};
    let mut ring = PcmRing::new(1, StreamRole::VirtualCapture.retention_frames()).unwrap();
    let input: Vec<f32> = (0..32_768)
        .map(|frame| f32::from_bits(0x3f00_0000 + frame))
        .collect();
    let mut output = vec![0.0; input.len()];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..2 {
            // SAFETY: the fixture serializes its sole producer with a complete native quantum.
            assert_eq!(
                unsafe { mr_pcm_ring_write(&raw mut ring, input.as_ptr(), 32_768) },
                32_768
            );
        }
        for _ in 0..2 {
            // SAFETY: the fixture owns the sole consumer and a whole writable native quantum.
            assert_eq!(
                unsafe { mr_pcm_ring_read(&raw mut ring, output.as_mut_ptr(), 32_768) },
                32_768
            );
            assert_eq!(output, input);
        }
    });
    assert_eq!(measurement.count_total, 0);
    assert_eq!(ring.stats().overflows, 0);
    assert_eq!(ring.stats().queued, 0);
}
