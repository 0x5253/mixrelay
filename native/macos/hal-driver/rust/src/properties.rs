//! `CoreAudio` properties are data/policy and therefore belong in Rust, not the
//! C vtable shim. `FourCC` values here are the stable Apple SDK ABI, not a second
//! product format catalog. Native ABI tests compare payloads with SDK structs.
mod device;
mod diagnostics;
pub(crate) mod ffi;
mod lock_free;
mod output_controls;
mod plugin;
mod settings;
mod status;
pub(crate) mod stream;
mod value;

#[cfg(test)]
mod tests;

use crate::registry::Registry;
use status::{BAD_OBJECT, ILLEGAL, UNKNOWN};
use value::Value;

#[repr(C)]
pub(crate) struct Address {
    pub selector: u32,
    pub scope: u32,
    pub element: u32,
}

pub(super) const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
pub(super) const INPUT: u32 = u32::from_be_bytes(*b"inpt");
pub(super) const OUTPUT: u32 = u32::from_be_bytes(*b"outp");

/// `kAudioObjectPlugInObject`.
const PLUGIN_OBJECT: u32 = 1;

/// Answers one property of any object the plug-in publishes.
fn property(
    registry: &Registry,
    object: u32,
    address: &Address,
    uid: Option<&str>,
) -> Result<Value, i32> {
    if object == PLUGIN_OBJECT {
        return plugin::property(registry, address, uid);
    }
    if address.element != 0 || ![GLOBAL, INPUT, OUTPUT].contains(&address.scope) {
        return Err(UNKNOWN);
    }
    if let Some(answer) = lock_free::property(registry, object, address) {
        return answer;
    }
    // Every other answer reads the control snapshot. An IO/timestamp thread must not wait
    // for control (StopIO holds it while draining that thread's pin), so it is refused.
    if crate::transport::on_audio_thread() {
        return Err(ILLEGAL);
    }
    if diagnostics::is_diagnostic(address.selector) {
        return diagnostics::property(registry, object, address);
    }
    let snapshot = registry.snapshot(object).ok_or(BAD_OBJECT)?;
    if (object == snapshot.input_stream_id && snapshot.capture_channels == 0)
        || (object == snapshot.output_stream_id && snapshot.render_channels == 0)
    {
        return Err(BAD_OBJECT);
    }
    if output_controls::is_control(&snapshot, object) {
        return output_controls::property(registry, &snapshot, object, address);
    }
    if let Some(value) = device::common(&snapshot, object, address.selector) {
        return Ok(value);
    }
    if object == snapshot.device_id {
        device::property(&snapshot, address)
    } else {
        stream::property(&snapshot, object, address)
    }
}
