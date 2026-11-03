use super::{MAX_CHANNELS, TapFormat, TapInputResult, TapPeaks, finite_magnitude};
use mixrelay_macos_transport::{
    audio_buffer::{AudioBuffer, AudioBufferList, buffers},
    capture_ring::{AudioRing, write_timed_interleaved, write_timed_planar},
};
use std::{ptr, slice};

fn frame_count(format: &TapFormat, parts: &[AudioBuffer]) -> Option<usize> {
    let first = parts.first()?;
    let channels = usize::try_from(format.channels).ok()?;
    let expected_parts = if format.planar() { channels } else { 1 };
    if !format.supported()
        || parts.len() != expected_parts
        || !first.byte_size.is_multiple_of(format.bytes_per_frame)
        || parts.iter().any(|part| {
            part.data.is_null()
                || !part.data.is_aligned()
                || part.byte_size != first.byte_size
                || part.channels != if format.planar() { 1 } else { format.channels }
        })
    {
        return None;
    }
    usize::try_from(first.byte_size / format.bytes_per_frame).ok()
}

/// # Safety
/// Native list and advertised descriptor extent remain readable during this callback.
pub(crate) unsafe fn input_frames(format: &TapFormat, input: *const AudioBufferList) -> usize {
    // SAFETY: caller retains native metadata; frame_count validates every selected part.
    unsafe { buffers(input, MAX_CHANNELS) }
        .and_then(|parts| frame_count(format, parts))
        .unwrap_or(0)
}

/// One channel's peak, reduced in independent lanes so the scan can vectorize; a single
/// running maximum would serialize every comparison.
pub(super) fn plane_peak(samples: &[f32]) -> f32 {
    const LANES: usize = 8;
    let mut lanes = [0.0_f32; LANES];
    let mut chunks = samples.chunks_exact(LANES);
    for chunk in &mut chunks {
        for (lane, &sample) in lanes.iter_mut().zip(chunk) {
            *lane = lane.max(finite_magnitude(sample));
        }
    }
    let tail = chunks
        .remainder()
        .iter()
        .fold(0.0, |peak: f32, &sample| peak.max(finite_magnitude(sample)));
    lanes.into_iter().fold(tail, f32::max)
}

// SAFETY: frame_count validated non-null/aligned native parts, equal complete
// Float32 extents and channels. CoreAudio retains readable bytes for this callback.
unsafe fn record(peaks: &TapPeaks, format: &TapFormat, parts: &[AudioBuffer]) -> bool {
    let mut block = [0.0_f32; MAX_CHANNELS];
    let channels = format.channels as usize;
    if format.planar() {
        for (peak, part) in block.iter_mut().zip(parts) {
            // SAFETY: each validated planar part contains byte_size / 4 floats.
            let samples = unsafe { slice::from_raw_parts(part.data, part.byte_size as usize / 4) };
            *peak = plane_peak(samples);
        }
    } else {
        let part = &parts[0];
        // SAFETY: validated interleaved extent consists of whole channel frames.
        let samples = unsafe { slice::from_raw_parts(part.data, part.byte_size as usize / 4) };
        // Frame-major, in memory order, so the scan stays sequential.
        for frame in samples.chunks_exact(channels) {
            for (peak, &sample) in block.iter_mut().zip(frame) {
                *peak = peak.max(finite_magnitude(sample));
            }
        }
    }
    peaks.publish(&block[..channels.min(MAX_CHANNELS)]);
    block[..channels.min(MAX_CHANNELS)]
        .iter()
        .any(|peak| *peak > 0.0)
}

/// Validates one tap block, records its peaks and writes its PCM without allocation.
/// Null/malformed input returns zero frames without reading PCM or changing meters.
/// # Safety
/// `MRProcessTap` retains the native list and its declared readable PCM extents for
/// the serialized `IOProc`. Format is immutable and matches the live ring's channel
/// width. Ring has this sole producer; peaks are initialized and owner-retained.
/// All buffers are disjoint from ring storage. No destruction overlaps this call.
pub(crate) unsafe fn ingest(
    format: &TapFormat,
    input: *const AudioBufferList,
    ring: *mut AudioRing,
    peaks: &TapPeaks,
    time: (u64, f64),
) -> TapInputResult {
    // SAFETY: CoreAudio supplies the readable descriptor table for this callback.
    let Some(parts) = (unsafe { buffers(input, MAX_CHANNELS) }) else {
        return TapInputResult::default();
    };
    let Some(frames) = frame_count(format, parts).filter(|frames| *frames != 0) else {
        return TapInputResult::default();
    };
    // SAFETY: frame_count validated geometry before any PCM dereference.
    let audible = unsafe { record(peaks, format, parts) };
    let written = if format.planar() {
        let mut planes = [ptr::null(); MAX_CHANNELS];
        for (plane, part) in planes.iter_mut().zip(parts) {
            *plane = part.data.cast_const();
        }
        // SAFETY: bounded table matches the ring's immutable channel width.
        unsafe { write_timed_planar(ring, planes.as_ptr(), frames, time.0, time.1) }
    } else {
        // SAFETY: validated interleaved extent is readable for frames*channels.
        unsafe { write_timed_interleaved(ring, parts[0].data, frames, time.0, time.1) }
    };
    TapInputResult {
        frames,
        written,
        audible,
    }
}
