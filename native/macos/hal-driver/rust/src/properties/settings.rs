//! Which properties the host may set, and how a set request is validated
//! (without the registry) and then applied.
use super::{
    Address, diagnostics,
    status::{BAD_OBJECT, BAD_SIZE, ILLEGAL, UNKNOWN, UNSUPPORTED_FORMAT},
    stream::{self, NATIVE_FORMATS},
};
use crate::{abi::Snapshot, output_control, registry::Registry};
use mixrelay_pcm_ring::SampleFormat;
use std::ffi::c_void;

const SETTABLE: [[u8; 4]; 8] = [
    *b"nsrt", *b"sact", *b"sfmt", *b"pft ", *b"lcsv", *b"lcdv", *b"bcvl", *b"mrdi",
];

/// Whether an existing property accepts a set; a missing one reports its get error.
pub(super) fn settable(registry: &Registry, object: u32, address: &Address) -> Result<bool, i32> {
    super::property(registry, object, address, None)?;
    Ok(SETTABLE.contains(&address.selector.to_be_bytes()))
}

/// Validates and applies one set request.
///
/// # Safety
/// `input` is non-null and readable for `size` bytes for this call.
pub(super) unsafe fn set(
    registry: &Registry,
    object: u32,
    address: &Address,
    size: u32,
    input: *const c_void,
) -> i32 {
    match settable(registry, object, address) {
        Ok(true) => {}
        Ok(false) => return ILLEGAL,
        Err(error) => return error,
    }
    if address.selector == diagnostics::ENABLED {
        // SAFETY: forwarded from this function's contract.
        return unsafe { diagnostics::set_enabled(registry, object, size, input) };
    }
    // SAFETY: forwarded from this function's contract.
    let bytes = unsafe { std::slice::from_raw_parts(input.cast::<u8>(), size as usize) };
    let Some(snapshot) = registry.snapshot(object) else {
        return BAD_OBJECT;
    };
    match Setting::parse(address.selector, bytes, stream::channels(&snapshot, object)) {
        Ok(setting) => setting.apply(registry, object, &snapshot),
        Err(error) => error,
    }
}

/// A validated set request, independent of the registry that applies it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Setting {
    /// Linear output gain; the registry validates its range.
    Volume(f32),
    Mute(bool),
    StreamActive(bool),
    Rate(u32),
    Format {
        rate: u32,
        encoding: SampleFormat,
    },
}

impl Setting {
    /// `channels` is the target stream's channel count, which a format must match.
    pub(super) fn parse(selector: u32, bytes: &[u8], channels: u32) -> Result<Self, i32> {
        match &selector.to_be_bytes() {
            // Output controls report every rejection as illegal, including the size.
            b"lcsv" | b"lcdv" | b"bcvl" => Self::output_control(selector, bytes).ok_or(ILLEGAL),
            b"sact" => {
                let bytes = <[u8; 4]>::try_from(bytes).map_err(|_| BAD_SIZE)?;
                Ok(Self::StreamActive(u32::from_ne_bytes(bytes) != 0))
            }
            b"nsrt" => {
                if bytes.len() != 8 {
                    return Err(BAD_SIZE);
                }
                parse_rate(bytes).map(Self::Rate).ok_or(UNSUPPORTED_FORMAT)
            }
            b"sfmt" | b"pft " => {
                if bytes.len() != 40 {
                    return Err(BAD_SIZE);
                }
                let rate = parse_rate(bytes).ok_or(UNSUPPORTED_FORMAT)?;
                NATIVE_FORMATS
                    .into_iter()
                    .find(|encoding| bytes == stream::format(rate, channels, *encoding))
                    .map(|encoding| Self::Format { rate, encoding })
                    .ok_or(UNSUPPORTED_FORMAT)
            }
            _ => Err(UNKNOWN),
        }
    }

    fn output_control(selector: u32, bytes: &[u8]) -> Option<Self> {
        let bytes = <[u8; 4]>::try_from(bytes).ok()?;
        match &selector.to_be_bytes() {
            b"bcvl" => Some(Self::Mute(u32::from_ne_bytes(bytes) != 0)),
            b"lcdv" => {
                output_control::decibels_to_scalar(f32::from_ne_bytes(bytes)).map(Self::Volume)
            }
            _ => Some(Self::Volume(f32::from_ne_bytes(bytes))),
        }
    }

    fn apply(self, registry: &Registry, object: u32, snapshot: &Snapshot) -> i32 {
        let status = match self {
            Self::Volume(gain) => registry.set_output_volume(object, gain),
            Self::Mute(muted) => registry.set_output_mute(object, muted),
            Self::StreamActive(active) => registry.set_stream_active(object, active),
            Self::Rate(rate) => registry.set_rate(snapshot.device_id, rate),
            Self::Format { rate, encoding } => registry.set_format(object, rate, encoding),
        };
        if status == 0 { 0 } else { ILLEGAL }
    }
}

/// A `Float64` rate that is exactly one of the supported rates.
fn parse_rate(bytes: &[u8]) -> Option<u32> {
    let value = f64::from_ne_bytes(bytes.get(..8)?.try_into().ok()?);
    crate::formats::SAMPLE_RATES
        .iter()
        .copied()
        .find(|rate| f64::from(*rate).to_bits() == value.to_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(selector: [u8; 4], bytes: &[u8]) -> Result<Setting, i32> {
        Setting::parse(u32::from_be_bytes(selector), bytes, 2)
    }

    #[test]
    fn rates_must_be_an_exact_supported_float64() {
        for &rate in crate::formats::SAMPLE_RATES {
            assert_eq!(
                parse(*b"nsrt", &f64::from(rate).to_ne_bytes()),
                Ok(Setting::Rate(rate))
            );
        }
        for value in [0.0, 48_000.5, 384_000.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                parse(*b"nsrt", &value.to_ne_bytes()),
                Err(UNSUPPORTED_FORMAT)
            );
        }
        assert_eq!(parse(*b"nsrt", &[0; 4]), Err(BAD_SIZE));
    }

    #[test]
    fn formats_must_match_a_catalog_description_for_the_stream_channels() {
        for encoding in NATIVE_FORMATS {
            assert_eq!(
                parse(*b"pft ", &stream::format(96_000, 2, encoding)),
                Ok(Setting::Format {
                    rate: 96_000,
                    encoding
                })
            );
        }
        let other_channels = stream::format(48_000, 4, SampleFormat::Float32);
        assert_eq!(parse(*b"sfmt", &other_channels), Err(UNSUPPORTED_FORMAT));
        let mut altered = stream::format(48_000, 2, SampleFormat::Signed24);
        altered[12] ^= 0x80;
        assert_eq!(parse(*b"sfmt", &altered), Err(UNSUPPORTED_FORMAT));
        let unsupported_rate = stream::format(8_000, 2, SampleFormat::Float32);
        assert_eq!(parse(*b"sfmt", &unsupported_rate), Err(UNSUPPORTED_FORMAT));
        assert_eq!(parse(*b"sfmt", &altered[..39]), Err(BAD_SIZE));
    }

    #[test]
    fn output_levels_convert_decibels_and_reject_everything_else_as_illegal() {
        let half = parse(*b"lcdv", &(-6.0206_f32).to_ne_bytes()).unwrap();
        assert!(matches!(half, Setting::Volume(gain) if (gain - 0.5).abs() < 0.0001));
        assert_eq!(
            parse(*b"lcsv", &0.25_f32.to_ne_bytes()),
            Ok(Setting::Volume(0.25))
        );
        assert_eq!(
            parse(*b"bcvl", &2_u32.to_ne_bytes()),
            Ok(Setting::Mute(true))
        );
        for value in [f32::NAN, -97.0, 1.0] {
            assert_eq!(parse(*b"lcdv", &value.to_ne_bytes()), Err(ILLEGAL));
        }
        assert_eq!(parse(*b"lcsv", &[0; 3]), Err(ILLEGAL));
        assert_eq!(parse(*b"bcvl", &[0; 8]), Err(ILLEGAL));
    }

    #[test]
    fn stream_activity_needs_one_u32_and_unknown_selectors_are_unknown() {
        assert_eq!(
            parse(*b"sact", &0_u32.to_ne_bytes()),
            Ok(Setting::StreamActive(false))
        );
        assert_eq!(parse(*b"sact", &[1; 2]), Err(BAD_SIZE));
        assert_eq!(parse(*b"lnam", &[0; 4]), Err(UNKNOWN));
    }
}
