use super::*;
use mixrelay_macos_transport::{
    audio_buffer::{AudioBuffer, AudioBufferList},
    capture_ring::{
        AudioRing, mr_audio_ring_create, mr_audio_ring_destroy, mr_audio_ring_dropped_frames,
        mr_audio_ring_read_interleaved,
    },
};

#[repr(C)]
struct BufferFixture {
    count: u32,
    parts: [AudioBuffer; MAX_CHANNELS],
}

impl BufferFixture {
    fn pointer(&self) -> *const AudioBufferList {
        std::ptr::from_ref(self).cast()
    }
}

struct RingOwner(*mut AudioRing);

impl RingOwner {
    fn new(channels: u32) -> Self {
        let ring = mr_audio_ring_create(8, channels);
        assert!(!ring.is_null());
        Self(ring)
    }
}

impl Drop for RingOwner {
    fn drop(&mut self) {
        // SAFETY: this test's sequential native calls have all returned.
        unsafe { mr_audio_ring_destroy(self.0) };
    }
}

fn format(channels: u32, planar: bool) -> TapFormat {
    let bytes_per_frame = 4 * if planar { 1 } else { channels };
    TapFormat {
        sample_rate: 48000.0,
        format_id: u32::from_be_bytes(*b"lpcm"),
        flags: FLOAT | PACKED | if planar { PLANAR } else { 0 },
        bytes_per_packet: bytes_per_frame,
        frames_per_packet: 1,
        bytes_per_frame,
        channels,
        bits: 32,
    }
}

fn fixture(samples: &mut [f32], channels: u32, planar: bool) -> BufferFixture {
    let frames = samples.len() / channels as usize;
    let mut fixture = BufferFixture {
        count: if planar { channels } else { 1 },
        parts: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: std::ptr::null_mut(),
        }; MAX_CHANNELS],
    };
    for (index, part) in fixture.parts[..fixture.count as usize]
        .iter_mut()
        .enumerate()
    {
        part.channels = if planar { 1 } else { channels };
        part.byte_size = u32::try_from(frames * part.channels as usize * 4).expect("small fixture");
        part.data = samples
            .as_mut_ptr()
            .wrapping_add(if planar { index * frames } else { 0 });
    }
    fixture
}

#[test]
fn format_validation_rejects_each_invalid_scalar_but_geometry_excludes_rate() {
    let valid = format(2, false);
    assert!(valid.supported());
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            !TapFormat {
                sample_rate: rate,
                ..valid
            }
            .supported()
        );
    }
    for invalid in [
        TapFormat {
            format_id: 0,
            ..valid
        },
        TapFormat {
            flags: FLOAT,
            ..valid
        },
        TapFormat {
            flags: PACKED,
            ..valid
        },
        TapFormat {
            flags: valid.flags | BIG_ENDIAN,
            ..valid
        },
        TapFormat {
            bytes_per_packet: 4,
            ..valid
        },
        TapFormat {
            frames_per_packet: 2,
            ..valid
        },
        TapFormat {
            bytes_per_frame: 4,
            ..valid
        },
        TapFormat {
            channels: 0,
            ..valid
        },
        TapFormat {
            channels: 65,
            ..valid
        },
        TapFormat { bits: 16, ..valid },
    ] {
        assert!(!invalid.supported());
        assert!(!valid.same_geometry(&invalid));
    }
    let changed = TapFormat {
        sample_rate: 44100.0,
        ..valid
    };
    assert!(changed.supported());
    assert!(valid.same_geometry(&changed));
}

#[test]
fn every_channel_width_preserves_pcm_and_finite_peak_policy_in_both_layouts() {
    for channels in 1..=64 {
        for planar in [false, true] {
            let format = format(channels, planar);
            let ring = RingOwner::new(channels);
            let peaks = TapPeaks::default();
            let mut samples = vec![0.0; channels as usize * 4];
            let values = [-0.5, 0.25, f32::from_bits(0x7fc0_1234), f32::INFINITY];
            for frame in 0..4 {
                for channel in 0..channels as usize {
                    samples[if planar {
                        channel * 4 + frame
                    } else {
                        frame * channels as usize + channel
                    }] = values[frame];
                }
            }
            let input = fixture(&mut samples, channels, planar);
            let mut output = vec![0.0; samples.len()];
            let mut observed_peaks = vec![0.0; channels as usize];
            // SAFETY: fixture owns complete aligned buffers and ring has matching width.
            unsafe {
                assert_eq!(
                    ingest(&format, input.pointer(), ring.0, &peaks, (0, 0.0)),
                    TapInputResult {
                        frames: 4,
                        written: 4,
                        audible: true,
                    }
                );
                assert_eq!(
                    mr_audio_ring_read_interleaved(ring.0, output.as_mut_ptr(), 4),
                    4
                );
                peaks.take(&mut observed_peaks);
            }
            for (frame, actual) in output.chunks_exact(channels as usize).enumerate() {
                assert!(
                    actual
                        .iter()
                        .all(|value| value.to_bits() == values[frame].to_bits())
                );
            }
            assert!(
                observed_peaks
                    .iter()
                    .all(|value| value.to_bits() == 0.5_f32.to_bits())
            );
            peaks.take(&mut observed_peaks);
            assert!(
                observed_peaks
                    .iter()
                    .all(|value| value.to_bits() == 0.0_f32.to_bits())
            );
        }
    }
}

#[test]
fn malformed_metadata_never_reads_pcm_or_publishes_partial_input() {
    let format = format(2, true);
    let ring = RingOwner::new(2);
    let peaks = TapPeaks::default();
    let mut samples = [0.5; 8];
    for mutation in 0..7 {
        let mut input = fixture(&mut samples, 2, true);
        match mutation {
            0 => input.count = 0,
            1 => input.count = 65,
            2 => input.count = 1,
            3 => input.parts[1].data = std::ptr::null_mut(),
            4 => input.parts[1].byte_size -= 1,
            5 => input.parts[1].channels = 2,
            6 => input.parts[1].data = std::ptr::without_provenance_mut(1),
            _ => unreachable!(),
        }
        // SAFETY: readable descriptor table, with bad sample pointers that must be rejected.
        assert_eq!(
            unsafe { ingest(&format, input.pointer(), ring.0, &peaks, (0, 0.0)) },
            TapInputResult::default()
        );
    }
    let mut output = [9.0; 2];
    peaks.take(&mut output);
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 2]);
    // SAFETY: live empty ring, readable empty input pointer is explicitly supported.
    unsafe {
        assert_eq!(mr_audio_ring_dropped_frames(ring.0), 0);
        assert_eq!(
            ingest(&format, std::ptr::null(), ring.0, &peaks, (0, 0.0)),
            TapInputResult::default()
        );
    }
}

#[test]
fn full_queue_still_observes_input_peaks_and_frames_without_allocating() {
    let format = format(2, false);
    let ring = RingOwner::new(2);
    let peaks = TapPeaks::default();
    let mut samples = [0.75; 16];
    let input = fixture(&mut samples, 2, false);
    let measured = allocation_counter::measure(|| {
        // SAFETY: all fixture storage and owners remain live, calls are serialized.
        unsafe {
            assert_eq!(
                ingest(&format, input.pointer(), ring.0, &peaks, (0, 0.0)).written,
                8
            );
            for _ in 0..1000 {
                peaks.clear();
                assert_eq!(
                    ingest(&format, input.pointer(), ring.0, &peaks, (0, 0.0)),
                    TapInputResult {
                        frames: 8,
                        written: 0,
                        audible: true,
                    }
                );
            }
        }
    });
    assert_eq!(measured.count_total, 0);
    let mut output = [0.0; 2];
    peaks.take(&mut output);
    assert_eq!(output.map(f32::to_bits), [0.75_f32.to_bits(); 2]);
    // SAFETY: all control/producer accesses have returned, ring is live.
    assert_eq!(unsafe { mr_audio_ring_dropped_frames(ring.0) }, 8000);
}

#[test]
fn diagnostic_audio_evidence_excludes_silence_and_nonfinite_samples() {
    for planar in [false, true] {
        let format = format(2, planar);
        let ring = RingOwner::new(2);
        let peaks = TapPeaks::default();
        let mut quiet = [0.0, -0.0, f32::NAN, f32::INFINITY];
        let quiet_input = fixture(&mut quiet, 2, planar);
        // SAFETY: each fixture owns complete buffers matching the ring's geometry.
        let result = unsafe { ingest(&format, quiet_input.pointer(), ring.0, &peaks, (0, 0.0)) };
        assert_eq!(result.frames, 2);
        assert!(!result.audible);
        let mut signal = [0.0, -f32::MIN_POSITIVE, 0.0, 0.0];
        let signal_input = fixture(&mut signal, 2, planar);
        // SAFETY: each fixture owns complete buffers matching the ring's geometry.
        let result = unsafe { ingest(&format, signal_input.pointer(), ring.0, &peaks, (0, 0.0)) };
        assert_eq!(result.frames, 2);
        assert!(result.audible);
    }
}

#[test]
fn peak_clear_preserves_output_bounds() {
    let peaks = TapPeaks::default();
    peaks.publish(&[finite_magnitude(-0.0), finite_magnitude(-2.0)]);
    // A smaller later block never lowers an undrained peak, and non-finite input is no level.
    peaks.publish(&[finite_magnitude(f32::NAN), finite_magnitude(-1.0)]);
    let mut output = [9.0; 65];
    peaks.take(&mut output);
    assert_eq!(output[0].to_bits(), 0.0_f32.to_bits());
    assert_eq!(output[1].to_bits(), 2.0_f32.to_bits());
    assert_eq!(output[64].to_bits(), 9.0_f32.to_bits());
    peaks.clear();
}

#[test]
fn plane_peaks_include_lanes_and_the_unaligned_tail() {
    assert_eq!(ingest::plane_peak(&[]).to_bits(), 0.0_f32.to_bits());
    // Eight lane samples, then a three-sample tail holding the peak; NaN never counts.
    let mut samples = [0.25; 11];
    samples[3] = f32::NAN;
    samples[9] = -0.875;
    assert_eq!(ingest::plane_peak(&samples).to_bits(), 0.875_f32.to_bits());
    samples[9] = 0.0;
    samples[5] = -0.5;
    assert_eq!(ingest::plane_peak(&samples).to_bits(), 0.5_f32.to_bits());
}
