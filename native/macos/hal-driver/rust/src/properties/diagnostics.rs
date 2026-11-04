//! The custom IO-trace properties: `mrdi` (enabled flag, settable) and
//! `mrdo` (trace snapshot), carried as `CFData`.
use super::{
    Address, GLOBAL,
    status::{BAD_OBJECT, UNKNOWN, data_error},
    value::Value,
};
use crate::registry::Registry;

pub(super) const ENABLED: u32 = u32::from_be_bytes(*b"mrdi");
const SNAPSHOT: u32 = u32::from_be_bytes(*b"mrdo");

pub(super) fn is_diagnostic(selector: u32) -> bool {
    selector == ENABLED || selector == SNAPSHOT
}

pub(super) fn property(registry: &Registry, object: u32, address: &Address) -> Result<Value, i32> {
    let device = registry.pin(object).ok_or(BAD_OBJECT)?;
    if device.id != object || address.scope != GLOBAL {
        return Err(UNKNOWN);
    }
    let _control = crate::device::lock(&device.control);
    if address.selector == ENABLED {
        Ok(Value::Data(
            u32::from(device.io_trace.enabled()).to_ne_bytes().to_vec(),
        ))
    } else {
        Ok(Value::Data(device.io_trace.snapshot()))
    }
}

/// Disables tracing while IO is frozen, then re-enables it from a reset trace.
///
/// # Safety
/// `input` is the host-marshalled `CFPropertyListRef` of `size` bytes.
pub(super) unsafe fn set_enabled(
    registry: &Registry,
    object: u32,
    size: u32,
    input: *const std::ffi::c_void,
) -> i32 {
    // SAFETY: the host marshals this custom property as a live CFPropertyListRef.
    let enabled = match unsafe { crate::property_data::read_flag(size, input) } {
        Ok(enabled) => enabled,
        Err(error) => return data_error(error),
    };
    let Some(device) = registry.pin(object) else {
        return BAD_OBJECT;
    };
    let _control = crate::device::lock(&device.control);
    let freeze = device.freeze();
    device.io_trace.set_enabled(false);
    drop(freeze);
    if enabled {
        device.io_trace.enable_after_reset();
    }
    0
}
