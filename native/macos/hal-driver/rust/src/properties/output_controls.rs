//! The output volume (`device + 3`) and mute (`device + 4`) control objects.
use super::{
    Address, GLOBAL, OUTPUT,
    status::{BAD_OBJECT, ILLEGAL, UNKNOWN},
    value::{Inline, Value, code, number, objects, pair, scalar, text},
};
use crate::{abi::Snapshot, output_control, registry::Registry};

pub(super) fn is_control(snapshot: &Snapshot, object: u32) -> bool {
    object == snapshot.device_id + 3 || object == snapshot.device_id + 4
}

pub(super) fn property(
    registry: &Registry,
    snapshot: &Snapshot,
    object: u32,
    address: &Address,
) -> Result<Value, i32> {
    if snapshot.render_channels == 0 {
        return Err(BAD_OBJECT);
    }
    if address.scope != GLOBAL {
        return Err(UNKNOWN);
    }
    let volume = object == snapshot.device_id + 3;
    let (gain, muted) = registry.output_control(object).ok_or(BAD_OBJECT)?;
    match &address.selector.to_be_bytes() {
        b"bcls" => Ok(code(if volume { *b"levl" } else { *b"togl" })),
        b"clas" => Ok(code(if volume { *b"vlme" } else { *b"mute" })),
        b"stdv" => Ok(number(snapshot.device_id)),
        b"lnam" => Ok(text(if volume {
            b"Output volume"
        } else {
            b"Output mute"
        })),
        b"ownd" => Ok(objects([])),
        b"cscp" => Ok(number(OUTPUT)),
        b"celm" => Ok(number(0)),
        b"lcsv" if volume => Ok(scalar(gain)),
        b"lcdv" if volume => Ok(scalar(
            output_control::scalar_to_decibels(gain).ok_or(ILLEGAL)?,
        )),
        b"lcdr" if volume => Ok(Value::Inline(Inline::new(pair::<8, 16>(
            f64::from(output_control::MIN_DECIBELS).to_ne_bytes(),
            0.0_f64.to_ne_bytes(),
        )))),
        b"lcsd" | b"lcds" if volume => Ok(scalar(0.0)),
        b"bcvl" if !volume => Ok(number(u32::from(muted))),
        _ => Err(UNKNOWN),
    }
}

/// Converts the in/out `Float32` of a level-conversion get (`lcsd`/`lcds`).
/// Other properties pass through unchanged.
///
/// # Safety
/// A non-null `output` must be readable and writable for `capacity` bytes.
pub(super) unsafe fn convert_level(
    value: Value,
    address: &Address,
    capacity: u32,
    output: *mut std::ffi::c_void,
) -> Result<Value, i32> {
    let convert = match &address.selector.to_be_bytes() {
        b"lcsd" => output_control::scalar_to_decibels,
        b"lcds" => output_control::decibels_to_scalar,
        _ => return Ok(value),
    };
    if capacity < 4 || output.is_null() {
        return Err(super::status::BAD_SIZE);
    }
    // SAFETY: CoreAudio defines these Get properties as an in/out Float32.
    // The caller supplies at least four readable/writable bytes for this call.
    let input = unsafe { output.cast::<f32>().read_unaligned() };
    convert(input).map(scalar).ok_or(ILLEGAL)
}
