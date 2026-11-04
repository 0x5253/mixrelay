//! Stream properties and the native formats a stream can carry.
use super::{
    Address, GLOBAL,
    status::{BAD_OBJECT, UNKNOWN},
    value::{Value, code, number, objects},
};
use crate::abi::Snapshot;
use mixrelay_pcm_ring::SampleFormat;

pub(crate) const NATIVE_FORMATS: [SampleFormat; 4] = [
    SampleFormat::Float32,
    SampleFormat::Signed16,
    SampleFormat::Signed24,
    SampleFormat::Signed32,
];

/// Builds the exact SDK ASBD for one interleaved little-endian native stream.
#[expect(
    clippy::cast_possible_truncation,
    reason = "SampleFormat samples occupy at most four bytes"
)]
pub(crate) fn format(rate: u32, channels: u32, encoding: SampleFormat) -> Vec<u8> {
    let sample_bytes = encoding.bytes_per_sample() as u32;
    let mut bytes = f64::from(rate).to_ne_bytes().to_vec();
    for field in [
        u32::from_be_bytes(*b"lpcm"),
        if encoding == SampleFormat::Float32 {
            9
        } else {
            12
        },
        channels * sample_bytes,
        1,
        channels * sample_bytes,
        channels,
        sample_bytes * 8,
        0,
    ] {
        bytes.extend_from_slice(&field.to_ne_bytes());
    }
    bytes
}

/// The channel count of `object`, a stream of the snapshot's device.
pub(super) fn channels(snapshot: &Snapshot, object: u32) -> u32 {
    if object == snapshot.input_stream_id {
        snapshot.capture_channels
    } else {
        snapshot.render_channels
    }
}

pub(super) fn property(snapshot: &Snapshot, object: u32, address: &Address) -> Result<Value, i32> {
    if address.scope != GLOBAL {
        return Err(UNKNOWN);
    }
    let input = object == snapshot.input_stream_id;
    let channels = channels(snapshot, object);
    if channels == 0 {
        return Err(BAD_OBJECT);
    }
    match &address.selector.to_be_bytes() {
        b"ownd" => Ok(objects([])),
        b"sact" => Ok(number(u32::from(if input {
            snapshot.input_active
        } else {
            snapshot.output_active
        }))),
        b"sdir" => Ok(number(u32::from(input))),
        b"term" => Ok(code(*b"line")),
        b"schn" => Ok(number(1)),
        b"ltnc" => Ok(number(0)),
        b"sfmt" | b"pft " => {
            let code = if input {
                snapshot.capture_format
            } else {
                snapshot.render_format
            };
            let encoding = NATIVE_FORMATS
                .into_iter()
                .find(|encoding| *encoding as u32 == code)
                .ok_or(BAD_OBJECT)?;
            Ok(Value::Struct(format(
                snapshot.sample_rate,
                channels,
                encoding,
            )))
        }
        b"sfma" | b"pfta" => Ok(available_formats(channels)),
        _ => Err(UNKNOWN),
    }
}

/// Every rate and encoding as an `AudioStreamRangedDescription`.
fn available_formats(channels: u32) -> Value {
    let mut bytes = Vec::new();
    for &rate in crate::formats::SAMPLE_RATES {
        for encoding in NATIVE_FORMATS {
            bytes.extend(format(rate, channels, encoding));
            bytes.extend(f64::from(rate).to_ne_bytes());
            bytes.extend(f64::from(rate).to_ne_bytes());
        }
    }
    Value::Array(bytes, 56)
}
