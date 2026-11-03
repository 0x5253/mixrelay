use super::*;

fn format(channels: u32, rate: f64, planar: bool) -> StreamFormat {
    StreamFormat {
        rate,
        format: u32::from_be_bytes(*b"lpcm"),
        flags: if planar { 33 } else { 1 },
        bits: 32,
        channels,
        frame_bytes: 4 * if planar { 1 } else { channels },
    }
}

fn selection(channels: u32, offset: u32, total: u32, limit: u32) -> BufferSelection {
    BufferSelection {
        channels,
        offset,
        total,
        limit,
    }
}

#[test]
fn device_formats_preserve_native_rates_without_imposing_tap_packet_rules() {
    for channels in 1..=64 {
        for rate in [8000, 44100, 48000, 96000, 384_000] {
            for planar in [false, true] {
                let mut state = StreamFormats::default();
                assert!(state.accept(&format(channels, f64::from(rate), planar), 64));
                assert_eq!(
                    state
                        .selected_rate(selection(channels, 0, 0, 64))
                        .map(f64::to_bits),
                    Some(f64::from(rate).to_bits())
                );
            }
        }
    }
}

#[test]
fn malformed_streams_do_not_change_the_accepted_prefix() {
    let valid = format(2, 48000.0, false);
    let invalid = [
        StreamFormat {
            rate: f64::NAN,
            ..valid
        },
        StreamFormat {
            rate: f64::INFINITY,
            ..valid
        },
        StreamFormat {
            rate: 48000.5,
            ..valid
        },
        StreamFormat {
            rate: 7999.0,
            ..valid
        },
        StreamFormat {
            rate: 384_001.0,
            ..valid
        },
        StreamFormat { format: 0, ..valid },
        StreamFormat { flags: 0, ..valid },
        StreamFormat { flags: 3, ..valid },
        StreamFormat { bits: 16, ..valid },
        StreamFormat {
            channels: 0,
            ..valid
        },
        StreamFormat {
            channels: 65,
            ..valid
        },
        StreamFormat {
            frame_bytes: 4,
            ..valid
        },
        StreamFormat {
            rate: 44100.0,
            ..valid
        },
    ];
    let mut state = StreamFormats::default();
    assert!(state.accept(&valid, 64));
    for value in invalid {
        assert!(!state.accept(&value, 64));
        assert_eq!(
            (state.channels, state.rate.to_bits()),
            (2, 48000_f64.to_bits())
        );
    }
}

#[test]
fn aggregate_totals_offsets_and_overflow_are_checked() {
    let mut state = StreamFormats::default();
    assert_eq!(state.selected_rate(selection(2, 0, 0, 64)), None);
    for _ in 0..32 {
        assert!(state.accept(&format(64, 96000.0, false), 2048));
    }
    assert_eq!(
        state
            .selected_rate(selection(64, 1984, 2048, 2048))
            .map(f64::to_bits),
        Some(96000_f64.to_bits())
    );
    assert_eq!(state.selected_rate(selection(64, 1985, 2048, 2048)), None);
    assert_eq!(state.selected_rate(selection(64, 0, 2047, 2048)), None);
    assert_eq!(state.selected_rate(selection(2, u32::MAX, 0, 2048)), None);
    assert_eq!(state.selected_rate(selection(0, 0, 0, 2048)), None);
    assert_eq!(state.selected_rate(selection(65, 0, 0, 2048)), None);
    assert_eq!(state.selected_rate(selection(2, 0, 0, 64)), None);
    assert!(!state.accept(&format(1, 96000.0, true), 2048));
    assert!(!state.accept(&format(1, 96000.0, true), 0));
    state.channels = u32::MAX;
    assert!(!state.accept(&format(1, 96000.0, true), 2048));
}

#[test]
fn admitted_format_query_is_allocation_free() {
    let mut state = StreamFormats::default();
    let facts = format(2, 44100.0, false);
    let measured = allocation_counter::measure(|| {
        assert!(state.accept(&facts, 64));
        assert_eq!(
            state
                .selected_rate(selection(2, 0, 0, 64))
                .map(f64::to_bits),
            Some(44100_f64.to_bits())
        );
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn native_pcm_formats_are_exact_packed_little_endian_streams() {
    for (bits, bytes, pcm) in [
        (16, 2, SampleFormat::Signed16),
        (24, 3, SampleFormat::Signed24),
        (32, 4, SampleFormat::Signed32),
    ] {
        for planar in [false, true] {
            let valid = StreamFormat {
                flags: 0x0c | if planar { 0x20 } else { 0x00 },
                bits,
                frame_bytes: bytes * if planar { 1 } else { 2 },
                ..format(2, 48000.0, planar)
            };
            let mut state = StreamFormats::default();
            assert_eq!(codec(&valid), Some(pcm));
            assert!(state.accept(&valid, 64));
            assert!(!state.into_layout().all_float());
            for flags in [
                valid.flags | 0x02,
                valid.flags | 0x10,
                valid.flags | 0x01,
                valid.flags & !0x08,
                valid.flags | 0x80,
            ] {
                let mut rejected = StreamFormats::default();
                assert!(!rejected.accept(&StreamFormat { flags, ..valid }, 64));
                assert_eq!(rejected.channels, 0);
            }
            let mut rejected = StreamFormats::default();
            assert!(!rejected.accept(
                &StreamFormat {
                    frame_bytes: valid.frame_bytes + 1,
                    ..valid
                },
                64
            ));
        }
    }
}
