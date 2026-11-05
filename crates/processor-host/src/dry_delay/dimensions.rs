//! Size limits checked with arithmetic alone, before any dry-delay history is allocated.

pub(super) const MAX_DRY_DELAY_BYTES: usize = 16 * 1024 * 1024;

/// Returns the flat-prefix stored channel count and its history sample count.
pub(super) fn checked_sizes(
    input_channels: usize,
    output_channels: usize,
    delay_frames: usize,
) -> Result<(usize, usize), &'static str> {
    if output_channels == 0 {
        return Err("dry-delay output channel count must be nonzero");
    }
    if delay_frames == 0 {
        return Err("dry-delay frame count must be nonzero");
    }
    let input_bytes = checked_sample_bytes(input_channels, delay_frames)
        .ok_or("dry-delay input stride size overflow")?;
    if input_bytes > MAX_DRY_DELAY_BYTES {
        return Err("dry-delay input stride exceeds the 16 MiB limit");
    }
    let output_bytes = checked_sample_bytes(output_channels, delay_frames)
        .ok_or("dry-delay output stride size overflow")?;
    if output_bytes > MAX_DRY_DELAY_BYTES {
        return Err("dry-delay output stride exceeds the 16 MiB limit");
    }
    let stored_channels = input_channels.min(output_channels);
    let history_samples = stored_channels
        .checked_mul(delay_frames)
        .ok_or("dry-delay history size overflow")?;
    // `stored_channels` is no greater than either side, so its history bytes are bounded by the
    // input/output stride caps above.
    Ok((stored_channels, history_samples))
}

fn checked_sample_bytes(channels: usize, frames: usize) -> Option<usize> {
    channels
        .checked_mul(frames)?
        .checked_mul(std::mem::size_of::<f32>())
}

#[cfg(test)]
mod tests {
    use super::{MAX_DRY_DELAY_BYTES, checked_sizes};

    #[test]
    fn overflowing_dimensions_are_rejected() {
        assert!(checked_sizes(usize::MAX, 1, 2).is_err());
        assert!(checked_sizes(0, usize::MAX, 2).is_err());
    }

    #[test]
    fn stride_and_history_limits_use_arithmetic_only() {
        let maximum_frames = MAX_DRY_DELAY_BYTES / std::mem::size_of::<f32>();
        assert_eq!(checked_sizes(1, 1, maximum_frames), Ok((1, maximum_frames)));
        assert!(checked_sizes(1, 1, maximum_frames + 1).is_err());
        assert_eq!(checked_sizes(0, 1, maximum_frames), Ok((0, 0)));
        assert!(checked_sizes(0, 1, maximum_frames + 1).is_err());
        assert!(checked_sizes(2, 1, maximum_frames / 2 + 1).is_err());
    }
}
