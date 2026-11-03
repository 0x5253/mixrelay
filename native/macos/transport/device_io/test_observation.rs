//! Hermetic observation of the production device owner. Explicit synthetic stream admission
//! invokes the production format validator off realtime, and the control entry points below
//! run the exact FFI preparation, listener and lifecycle code beneath a [`SimulatedHal`].

use super::{
    DeviceIo, control, ffi,
    native::{self, BufferInfo},
    stream_format::{StreamFormat, StreamFormats},
};
use crate::aggregate_group::NativeBinding;

mod simulated_hal;
pub(crate) use simulated_hal::Composition;
pub use simulated_hal::{DEFAULT_BUFFER, SimulatedHal, Stream};

/// Admits one interleaved packed Float32 stream matching a synthetic owner's declared geometry.
///
/// Returns `false` for null owners, registered callbacks or unsupported rates/layouts.
/// Real device preparation still requires actual `CoreAudio` stream descriptions and listeners.
///
/// # Safety
/// `io` is null or a retained, freshly created fixture owner that has never registered native I/O.
/// Its sole fixture controller excludes callbacks and graph/worker queue access during admission.
/// It supplies complete matching Float32 ABL storage to subsequent serialized callback invocations.
#[must_use]
pub unsafe fn admit_float_callback(io: *mut DeviceIo) -> bool {
    // SAFETY: fixture owns the stable allocation throughout this control-only admission.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return false;
    };
    // SAFETY: no callback or other control owner accesses these cells during preparation.
    let control = unsafe { &*io.control.get() };
    if control.lifecycle.registered() {
        return false;
    }
    // SAFETY: this never-registered fixture exclusively owns its callback cell.
    let callback = unsafe { &mut *io.callback.get() };
    let mut formats = StreamFormats::default();
    if !formats.accept(
        &StreamFormat {
            rate: f64::from(control.connection.rate),
            format: u32::from_be_bytes(*b"lpcm"),
            flags: 0x09,
            bits: 32,
            channels: io.selection.channels,
            frame_bytes: io.selection.channels * 4,
        },
        callback.selection.limit,
    ) || formats.selected_rate(callback.selection).is_none()
    {
        return false;
    }
    callback.formats = formats.into_layout();
    true
}

/// `mr_device_io_connect_status` beneath `hal`.
/// # Safety
/// `io` is null or retained; the sole control owner excludes other control calls.
pub unsafe fn connect_status(hal: &SimulatedHal, io: *mut DeviceIo, expected: u32) -> i32 {
    // SAFETY: stable context retained by its one control owner; only control cell mutates.
    unsafe { io.as_ref() }.map_or(-2, |io| unsafe { control::connect(io, expected, hal) })
}

/// `mr_device_io_disconnect` beneath `hal`.
/// # Safety
/// Sole control owner retains `io` and excludes connect/disconnect/destruction calls.
pub unsafe fn disconnect(hal: &SimulatedHal, io: *mut DeviceIo) -> bool {
    // SAFETY: callbacks touch only disjoint callback cells/queues/atomics.
    unsafe { io.as_ref() }.is_none_or(|io| unsafe { control::disconnect(io, hal) })
}

/// `mr_device_io_destroy` beneath `hal`: false retains the owner for retry.
/// # Safety
/// The caller is the sole allocation owner and stopped worker/graph readers; on success
/// `io` is never used again.
pub unsafe fn destroy(hal: &SimulatedHal, io: *mut DeviceIo) -> bool {
    // SAFETY: the outer allocation owner has stopped graph/worker roles.
    unsafe { ffi::destroy_with(io, |io| control::disconnect(io, hal)) }
}

/// `mr_device_io_alive` beneath `hal`.
/// # Safety
/// The single control owner excludes connection state changes during this call.
pub unsafe fn alive(hal: &SimulatedHal, io: *mut DeviceIo) -> bool {
    // SAFETY: control-only query never touches callback scratch or queue cursors.
    unsafe { io.as_ref() }.is_some_and(|io| unsafe { control::alive(io, hal) })
}

/// `mr_device_io_clock_device` beneath `hal`.
/// # Safety
/// Sole control owner excludes connection writes; the aggregate owner remains retained.
pub unsafe fn clock_device(hal: &SimulatedHal, io: *mut DeviceIo) -> u32 {
    // SAFETY: serialized control reads only immutable selection and connection cell.
    unsafe { io.as_ref() }.map_or(0, |io| unsafe { control::clock_device(io, hal) })
}

/// `mr_device_io_direct_binding` beneath `hal`.
/// # Safety
/// `io` is null or retained by its owner for this immutable-selection query.
pub unsafe fn direct_binding(hal: &SimulatedHal, io: *mut DeviceIo) -> Option<NativeBinding> {
    // SAFETY: immutable selection remains live while the engine retains IO.
    control::direct_binding(unsafe { io.as_ref() }?, hal)
}

/// `mr_device_io_buffer_info` beneath `hal`; false leaves a non-null `info` zeroed.
/// # Safety
/// `io` is null or retained, and `info` is null or exclusive aligned storage disjoint from it.
pub unsafe fn buffer_info(hal: &SimulatedHal, io: *mut DeviceIo, info: *mut BufferInfo) -> bool {
    if info.is_null() {
        return false;
    }
    // SAFETY: immutable selection remains live while the engine retains IO.
    let snapshot = unsafe { io.as_ref() }.and_then(|io| control::buffer_info(io, hal));
    // SAFETY: the caller grants complete, aligned, exclusive output storage.
    unsafe { info.write(snapshot.unwrap_or_default()) };
    snapshot.is_some()
}

/// `mr_device_buffer_size` beneath `hal`.
#[must_use]
pub fn buffer_size(hal: &SimulatedHal, device: u32) -> u32 {
    native::buffer_size(device, hal)
}

/// `mr_device_buffer_write` beneath `hal`.
pub fn buffer_write(hal: &SimulatedHal, device: u32, frames: u32) -> bool {
    native::buffer_write(device, frames, hal)
}

#[cfg(test)]
mod tests;
