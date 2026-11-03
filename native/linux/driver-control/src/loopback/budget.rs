//! Validates worst-case RAW work before advancing the rate mapper or PCM owners.
use mixrelay_pipewire::{DriverCycle, DriverProcessFailure, StreamFormat};

pub(super) fn failure(native: &DriverCycle, stream_rate: u32) -> DriverProcessFailure {
    DriverProcessFailure::FrameBudget {
        native_frames: native.frames,
        native_rate: native.rate,
        stream_rate,
        capacity_frames: StreamFormat::MAX_TRANSFER_FRAMES,
    }
}

pub(super) fn validate(native: &DriverCycle, stream_rate: u32) -> Result<(), DriverProcessFailure> {
    if !(8_000..=384_000).contains(&native.rate)
        || !(8_000..=384_000).contains(&stream_rate)
        || !(1..=DriverCycle::MAX_FRAMES).contains(&native.frames)
    {
        return Err(failure(native, stream_rate));
    }
    // Carry can add one frame; reserve the ceiling before the mapper consumes any clock state.
    let maximum =
        (u64::from(native.frames) * u64::from(stream_rate)).div_ceil(u64::from(native.rate));
    if maximum > u64::from(StreamFormat::MAX_TRANSFER_FRAMES) {
        return Err(failure(native, stream_rate));
    }
    Ok(())
}
