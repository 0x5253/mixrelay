//! Shared product policy and allocation geometry. Native negotiation may narrow
//! these choices; it must not turn a product choice into a hardware guarantee.
use crate::formats::{BUFFER_SIZES, SAMPLE_RATES};

#[cfg(test)]
#[path = "../native_format_header.rs"]
mod native_format_header;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    #[error("unsupported sample rate: {0}")]
    SampleRate(u32),
    #[error("unsupported buffer size: {0}")]
    BufferSize(u32),
    #[error("invalid audio block geometry")]
    BlockGeometry,
}

/// Checks product choices before a backend applies its native constraints.
///
/// # Errors
/// Rejects values outside the shared product catalog.
pub fn validate_format(sample_rate: u32, block_frames: u32) -> Result<(), FormatError> {
    if !SAMPLE_RATES.contains(&sample_rate) {
        return Err(FormatError::SampleRate(sample_rate));
    }
    if !BUFFER_SIZES.contains(&block_frames) {
        return Err(FormatError::BufferSize(block_frames));
    }
    Ok(())
}

/// Intersects native support with product policy, in stable catalog order.
#[must_use]
pub fn supported_rates(mut native_supports: impl FnMut(u32) -> bool) -> Vec<u32> {
    SAMPLE_RATES
        .iter()
        .copied()
        .filter(|rate| native_supports(*rate))
        .collect()
}

/// Checks dimensions separately before multiplying. Callback frames may be a
/// partial block, so this is a storage bound, not a settings-catalog check.
///
/// # Errors
/// Rejects empty blocks, out-of-range dimensions and address-space overflow.
pub fn block_sample_count(
    channels: u32,
    frames: u32,
    max_channels: u32,
    max_frames: u32,
) -> Result<usize, FormatError> {
    if channels == 0 || frames == 0 || channels > max_channels || frames > max_frames {
        return Err(FormatError::BlockGeometry);
    }
    usize::try_from(channels)
        .ok()
        .zip(usize::try_from(frames).ok())
        .and_then(|(channels, frames)| channels.checked_mul(frames))
        .ok_or(FormatError::BlockGeometry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_product_combination_is_valid() {
        assert_eq!(
            SAMPLE_RATES,
            [44_100, 48_000, 88_200, 96_000, 176_400, 192_000]
        );
        assert_eq!(BUFFER_SIZES, [32, 64, 128, 256, 512, 1024, 2048]);
        for &rate in SAMPLE_RATES {
            for &frames in BUFFER_SIZES {
                assert_eq!(validate_format(rate, frames), Ok(()));
            }
        }
        for rate in [0, 48_001, 352_800, 384_000, u32::MAX] {
            assert_eq!(
                validate_format(rate, 128),
                Err(FormatError::SampleRate(rate))
            );
        }
        for frames in [0, 16, 33, 4096, u32::MAX] {
            assert_eq!(
                validate_format(48_000, frames),
                Err(FormatError::BufferSize(frames))
            );
        }
    }

    #[test]
    fn native_declarations_use_the_exact_rust_catalog() {
        let header = native_format_header::render();
        assert!(header.contains("MR_AUDIO_SAMPLE_RATE_COUNT = 6"));
        let declaration = header
            .lines()
            .find(|line| line.starts_with("static const uint32_t"))
            .unwrap();
        let rates = declaration
            .split_once("= {")
            .unwrap()
            .1
            .trim_end_matches("};")
            .split(", ")
            .map(|rate| rate.trim_end_matches('U').parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rates, SAMPLE_RATES);
        assert!(!header.contains("384000"));
    }

    #[test]
    fn native_header_emission_is_confined_to_the_build_directory() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/format-tests")
            .join(format!(
                "mixrelay-format-header-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let header = native_format_header::write_header(&directory).unwrap();
        assert_eq!(header.parent(), Some(directory.as_path()));
        assert_eq!(
            std::fs::read_to_string(&header).unwrap(),
            native_format_header::render()
        );
        // An existing file cannot be treated as a directory. Generation fails
        // explicitly instead of writing into source or another fallback path.
        assert!(native_format_header::write_header(&header).is_err());
        std::fs::remove_file(header).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn discovery_can_narrow_but_never_expand_the_catalog() {
        assert_eq!(supported_rates(|_| true), SAMPLE_RATES);
        assert!(supported_rates(|_| false).is_empty());
        assert_eq!(
            supported_rates(|rate| [48_000, 384_000].contains(&rate)),
            [48_000]
        );
    }

    #[test]
    fn small_channel_counts_cannot_bypass_the_frame_limit() {
        assert_eq!(block_sample_count(64, 2048, 64, 2048), Ok(131_072));
        assert_eq!(block_sample_count(1, 7, 64, 2048), Ok(7));
        for (channels, frames) in [(0, 32), (1, 0), (65, 32), (1, 2049), (u32::MAX, u32::MAX)] {
            assert_eq!(
                block_sample_count(channels, frames, 64, 2048),
                Err(FormatError::BlockGeometry)
            );
        }
        assert_eq!(
            block_sample_count(u32::MAX, u32::MAX, u32::MAX, u32::MAX).ok(),
            usize::try_from(u32::MAX)
                .ok()
                .and_then(|n| n.checked_mul(n))
        );
    }
}
