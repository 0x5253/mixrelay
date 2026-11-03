//! Run this measurement alone in release mode with `--test-threads=1`.
//! Same PCM, widths, capacity, iterations and pass counts as the removed native harness.

use super::{RingOwner, TapFormat, TapSession, ffi};
use mixrelay_macos_transport::{
    audio_buffer::{AudioBuffer, AudioBufferList},
    capture_ring::{mr_audio_ring_create, mr_audio_ring_read_interleaved},
};
use std::{ptr, sync::atomic::Ordering, time::Instant};

#[repr(C)]
struct Buffers {
    count: u32,
    parts: [AudioBuffer; 64],
}

fn input(samples: &mut [f32], channels: u32, frames: u32, planar: bool) -> Buffers {
    let count = if planar { channels } else { 1 };
    let mut input = Buffers {
        count,
        parts: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: ptr::null_mut(),
        }; 64],
    };
    for (index, part) in input.parts[..count as usize].iter_mut().enumerate() {
        *part = AudioBuffer {
            channels: if planar { 1 } else { channels },
            byte_size: 4 * frames * if planar { 1 } else { channels },
            data: samples.as_mut_ptr().wrapping_add(if planar {
                index * frames as usize
            } else {
                0
            }),
        };
    }
    input
}

fn run(channels: u32, frames: u32, planar: bool) {
    let session = TapSession::default();
    let width = if planar { 1 } else { channels };
    session
        .format
        .set(TapFormat {
            sample_rate: 48_000.0,
            format_id: u32::from_be_bytes(*b"lpcm"),
            flags: 9 | if planar { 32 } else { 0 },
            bytes_per_packet: 4 * width,
            frames_per_packet: 1,
            bytes_per_frame: 4 * width,
            channels,
            bits: 32,
        })
        .expect("unpublished benchmark format");
    let ring = mr_audio_ring_create(16_384, channels);
    assert!(!ring.is_null());
    session
        .ring
        .set(RingOwner(ring))
        .expect("unpublished benchmark ring");
    session.delivery.store(true, Ordering::Release);
    session
        .sample_rate
        .store(48_000_f64.to_bits(), Ordering::Release);
    let mut samples = vec![0.25; channels as usize * frames as usize];
    let mut output = vec![0.0; samples.len()];
    let input = input(&mut samples, channels, frames, planar);
    let mut elapsed = [0.0_f64; 7];
    for pass in 0..8 {
        let begin = Instant::now();
        for _ in 0..20_000 {
            // SAFETY: each iteration supplies the retained, initialized native list;
            // exactly one producer and one sequential consumer use this live ring.
            unsafe {
                ffi::io_proc(
                    0,
                    ptr::null(),
                    ptr::from_ref(&input).cast::<AudioBufferList>(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::from_ref(&session).cast_mut().cast(),
                );
                assert_eq!(
                    mr_audio_ring_read_interleaved(ring, output.as_mut_ptr(), frames as usize),
                    frames as usize
                );
            }
        }
        let duration = begin.elapsed();
        if pass != 0 {
            elapsed[pass - 1] = duration.as_secs_f64() * 1e9 / 20_000.0;
        }
        assert!(
            output
                .iter()
                .zip(&samples)
                .all(|(left, right)| left.to_bits() == right.to_bits())
        );
    }
    elapsed.sort_by(f64::total_cmp);
    eprintln!(
        "channels={channels} frames={frames} planar={} ns/callback+drain min={:.3} median={:.3} max={:.3}",
        u8::from(planar),
        elapsed[0],
        elapsed[3],
        elapsed[6]
    );
}

#[test]
fn callback_and_drain() {
    for channels in [2, 8, 64] {
        for frames in [32, 256] {
            for planar in [false, true] {
                run(channels, frames, planar);
            }
        }
    }
}
