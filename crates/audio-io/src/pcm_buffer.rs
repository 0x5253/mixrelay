//! Fallible PCM and packet-storage preparation outside the realtime path.
use std::collections::TryReserveError;

/// Reserves storage for samples or their packet metadata before publication.
///
/// # Errors
/// Returns a reservation error; no partially prepared storage escapes.
pub fn reserve<T>(elements: usize) -> Result<Vec<T>, TryReserveError> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(elements)?;
    Ok(buffer)
}

/// Prepares cleared interleaved or planar PCM scratch outside realtime.
///
/// # Errors
/// Returns a reservation error without publishing partially prepared storage.
pub fn zeroed(samples: usize) -> Result<Vec<f32>, TryReserveError> {
    let mut buffer = reserve(samples)?;
    buffer.resize(samples, 0.0);
    Ok(buffer)
}

/// Prepares cleared per-frame source coordinates outside realtime.
/// # Errors
/// Returns a reservation error before any stream state is published.
pub fn timestamps(frames: usize) -> Result<Vec<u64>, TryReserveError> {
    let mut buffer = reserve(frames)?;
    buffer.resize(frames, 0);
    Ok(buffer)
}
