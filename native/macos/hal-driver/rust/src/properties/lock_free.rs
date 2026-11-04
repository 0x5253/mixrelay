//! Answers that IO and timestamp threads may request reentrantly. They read
//! only fixed-slot pins and atomics and return inline values: no control lock,
//! snapshot or allocation. `StopIO` holds the control mutex while it waits for
//! a callback's IO pin to drain, so waiting for control here would deadlock.
use super::{
    Address, GLOBAL, INPUT, OUTPUT,
    status::{BAD_OBJECT, UNKNOWN},
    value::{Value, number},
};
use crate::registry::Registry;
use std::sync::atomic::Ordering;

const LATENCY: u32 = u32::from_be_bytes(*b"ltnc");
const SAFETY_OFFSET: u32 = u32::from_be_bytes(*b"saft");
pub(super) const TRANSPORT: u32 = u32::from_be_bytes(*b"tran");

/// The lock-free answer for `address`, or `None` when it needs the snapshot.
pub(super) fn property(
    registry: &Registry,
    object: u32,
    address: &Address,
) -> Option<Result<Value, i32>> {
    if let Some(value) = crate::clock::property(address.selector) {
        return Some(clock(registry, object, value));
    }
    match address.selector {
        LATENCY | SAFETY_OFFSET => Some(latency(registry, object, address)),
        TRANSPORT => Some(transport(registry, object)),
        _ => None,
    }
}

/// The clock's constant metadata, answered for device objects only.
fn clock(registry: &Registry, object: u32, value: u32) -> Result<Value, i32> {
    let device = registry.pin(object).ok_or(BAD_OBJECT)?;
    (object == device.id).then(|| number(value)).ok_or(UNKNOWN)
}

/// Device latency is directional and includes the effective input latency;
/// stream latency is zero for streams that exist.
fn latency(registry: &Registry, object: u32, address: &Address) -> Result<Value, i32> {
    let device = registry.pin(object).ok_or(BAD_OBJECT)?;
    if object == device.id {
        if ![INPUT, OUTPUT].contains(&address.scope) {
            return Err(UNKNOWN);
        }
        return Ok(number(
            if address.scope == INPUT && address.selector == LATENCY {
                device
                    .input_latency
                    .effective(device.rate.load(Ordering::Acquire))
            } else {
                0
            },
        ));
    }
    if address.selector != LATENCY || object > device.id + 2 || address.scope != GLOBAL {
        return Err(UNKNOWN);
    }
    let channels = if object == device.id + 1 {
        &device.capture_channels
    } else {
        &device.render_channels
    };
    if channels.load(Ordering::Acquire) == 0 {
        return Err(BAD_OBJECT);
    }
    Ok(number(0))
}

/// The device transport type; per-requester presentation is applied on get.
fn transport(registry: &Registry, object: u32) -> Result<Value, i32> {
    let device = registry.pin(object).ok_or(BAD_OBJECT)?;
    if (object == device.id + 1 && device.capture_channels.load(Ordering::Acquire) == 0)
        || (object >= device.id + 2 && device.render_channels.load(Ordering::Acquire) == 0)
    {
        return Err(BAD_OBJECT);
    }
    if object == device.id {
        Ok(number(crate::transport::VIRTUAL))
    } else {
        Err(UNKNOWN)
    }
}
