//! Control-owner registration over one injectable `CoreAudio` boundary. Production uses
//! [`System`]; hermetic tests supply a simulated HAL beneath the same preparation,
//! listener and lifecycle logic.
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Release},
        },
    },
};

use super::{
    Connection, DeviceIo,
    listeners::{ListenerBackend, Listeners, WatchTarget},
    native::{self, BufferInfo, Properties, System},
    stream_usage,
};
use crate::{
    aggregate_group::NativeBinding,
    callback_clock::mr_callback_clock_reset,
    device_io::lifecycle::{Backend, Created},
};

/// The `CoreAudio` calls of one owner's registration: properties, listeners and its `IOProc`.
pub(super) trait Hal: Properties + ListenerBackend + stream_usage::Backend {
    /// Creates this crate's `IOProc` on `device` with `context`; a partial handle transfers.
    ///
    /// # Safety
    /// `context` is a retained [`DeviceIo`] allocation that outlives the registration until
    /// [`Hal::destroy`] acknowledges it.
    unsafe fn create(&self, device: u32, context: NonNull<c_void>) -> Created;
    fn start(&self, device: u32, procedure: NonNull<c_void>) -> i32;
    fn stop(&self, device: u32, procedure: NonNull<c_void>) -> i32;
    fn destroy(&self, device: u32, procedure: NonNull<c_void>) -> i32;
}

impl Hal for System {
    unsafe fn create(&self, device: u32, context: NonNull<c_void>) -> Created {
        let mut procedure = ptr::null_mut();
        // SAFETY: the caller retains the stable context even on partial creation; the
        // callback accesses only its cell, queues and atomic publication fields.
        let status = unsafe {
            native::create(
                device,
                super::mr_device_io_process,
                context.as_ptr(),
                &raw mut procedure,
            )
        };
        Created {
            procedure: NonNull::new(procedure),
            status,
        }
    }

    fn start(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        // SAFETY: CoreAudio validates the device and procedure identities it issued.
        unsafe { native::start(device, procedure.as_ptr()) }
    }

    fn stop(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        // SAFETY: registration and allocation remain retained regardless of stop status.
        unsafe { native::stop(device, procedure.as_ptr()) }
    }

    fn destroy(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        // SAFETY: stop attempted; only lifecycle acknowledges confirmed native release.
        unsafe { native::destroy(device, procedure.as_ptr()) }
    }
}

struct Registration<'a, H> {
    io: &'a DeviceIo,
    connection: &'a mut Connection,
    context: NonNull<c_void>,
    listeners: &'a mut Listeners,
    hal: &'a H,
}

impl<H> Registration<'_, H> {
    fn unused_scope(&self) -> Option<u32> {
        if self.io.selection.input {
            Some(u32::from_be_bytes(*b"outp"))
        } else {
            self.io
                .progress
                .output_only
                .load(Acquire)
                .then_some(u32::from_be_bytes(*b"inpt"))
        }
    }
}

impl<H: Hal> Backend for Registration<'_, H> {
    fn prepare(&mut self, expected: u32) -> Result<u32, i32> {
        let binding = native::resolve(&self.io.selection, self.hal).ok_or(-2)?;
        if expected != 0 && binding.device != expected {
            return Err(-5);
        }
        if !self.listeners.remove(self.hal) {
            return Err(-6);
        }
        // A fresh registration owns its latch. Late copies of removed native blocks cannot
        // invalidate the next registration. Metadata invalidation is reset only after drain.
        self.io.progress.invalid.store(false, Release);
        let watch_invalid = Arc::new(AtomicBool::new(false));
        self.listeners.install(
            &WatchTarget {
                device: binding.device,
                scope: u32::from_be_bytes(if self.io.selection.input {
                    *b"inpt"
                } else {
                    *b"outp"
                }),
                limit: if self.io.selection.group.is_null() {
                    64
                } else {
                    2048
                },
                unused_scope: self.unused_scope(),
            },
            &watch_invalid,
            self.hal,
        )?;
        let format = native::admitted_format(&self.io.selection, binding, self.hal).ok_or(-3)?;
        if watch_invalid.load(Acquire)
            || native::resolve(&self.io.selection, self.hal) != Some(binding)
        {
            return Err(-6);
        }
        // SAFETY: lifecycle has no registration during prepare. The engine
        // control owner has acknowledged drain before reusing callback state.
        unsafe { adopt(self.io, self.connection, binding, format, watch_invalid) };
        Ok(binding.device)
    }

    fn create(&mut self, device: u32) -> Created {
        // SAFETY: the engine retains this stable allocation until destroy acknowledges.
        unsafe { self.hal.create(device, self.context) }
    }

    fn start(&mut self, device: u32, procedure: NonNull<c_void>) -> i32 {
        if self.io.progress.invalid.load(Acquire) || self.connection.watch_invalid.load(Acquire) {
            return -6;
        }
        if self.unused_scope().is_some() {
            let status = stream_usage::disable_unused(
                self.hal,
                device,
                procedure,
                self.io.selection.input,
                if self.io.selection.group.is_null() {
                    64
                } else {
                    2048
                },
            );
            if status != 0 {
                return status;
            }
        }
        if self.io.progress.invalid.load(Acquire) || self.connection.watch_invalid.load(Acquire) {
            return -6;
        }
        self.hal.start(device, procedure)
    }

    fn silence(&mut self) {
        self.io.progress.invalid.store(true, Release);
    }

    fn stop(&mut self, device: u32, procedure: NonNull<c_void>) -> i32 {
        self.hal.stop(device, procedure)
    }

    fn destroy(&mut self, device: u32, procedure: NonNull<c_void>) -> i32 {
        self.hal.destroy(device, procedure)
    }
}

/// Publishes one admitted native stream to the connection and a reset callback state.
/// # Safety
/// No callback is registered and the previous registration has drained; the sole
/// control owner excludes concurrent connection and callback-cell access.
unsafe fn adopt(
    io: &DeviceIo,
    connection: &mut Connection,
    binding: NativeBinding,
    format: native::AdmittedFormat,
    watch_invalid: Arc<AtomicBool>,
) {
    *connection = Connection {
        binding,
        rate: format.rate,
        formats: format.layout.clone(),
        watch_invalid: watch_invalid.clone(),
    };
    // SAFETY: no registration exists, so this control owner exclusively borrows the cell.
    let callback = unsafe { &mut *io.callback.get() };
    callback.selection = format.selection;
    callback.formats = format.layout;
    callback.watch_invalid = watch_invalid;
    callback.prime = true;
    callback.frame_nanos = 1e9 / f64::from(connection.rate);
    callback.frame_ticks = 0.0;
    callback.previous_timestamp = None;
    io.execution.reset();
    io.progress
        .frame_nanos
        .store(callback.frame_nanos.to_bits(), Release);
    // SAFETY: the preceding registration drained; atomic observers may remain.
    unsafe { mr_callback_clock_reset(ptr::from_ref(&io.clock).cast_mut()) };
}

/// Registers and starts the owner's `IOProc` through `hal`: zero, or a preflight/native status.
/// # Safety
/// One engine control worker excludes other control calls for this owner. Callbacks do
/// not touch Control, so borrowing that cell does not alias their state.
pub(super) unsafe fn connect(io: &DeviceIo, expected: u32, hal: &impl Hal) -> i32 {
    // SAFETY: serialized control access; no whole-DeviceIo mutable reference is formed.
    let control = unsafe { &mut *io.control.get() };
    let context = NonNull::from(io).cast();
    let status = control.lifecycle.connect(
        expected,
        &mut Registration {
            io,
            connection: &mut control.connection,
            context,
            listeners: &mut control.listeners,
            hal,
        },
    );
    if status != 0 && !control.lifecycle.registered() {
        io.progress.invalid.store(true, Release);
        control.listeners.remove(hal);
    }
    status
}

/// Stops and unregisters through `hal`; false retains the registration for another attempt.
/// # Safety
/// Sole control worker; the owner remains allocated on a failed unregister.
pub(super) unsafe fn disconnect(io: &DeviceIo, hal: &impl Hal) -> bool {
    // SAFETY: callback borrows only disjoint cells, queues and atomics.
    let control = unsafe { &mut *io.control.get() };
    let drained = control.lifecycle.disconnect(&mut Registration {
        io,
        connection: &mut control.connection,
        context: NonNull::from(io).cast(),
        listeners: &mut control.listeners,
        hal,
    });
    if !drained {
        return false;
    }
    io.progress.invalid.store(true, Release);
    control.listeners.remove(hal)
}

/// The owner's current all-Float32 projection for the direct render path. Reads only
/// immutable selection, never the worker's mutable connection.
pub(super) fn direct_binding(io: &DeviceIo, properties: &impl Properties) -> Option<NativeBinding> {
    let binding = native::resolve(&io.selection, properties)?;
    native::admitted_format(&io.selection, binding, properties)
        .is_some_and(|format| format.layout.all_float())
        .then_some(binding)
}

/// The owner's resolved client buffer facts, from immutable selection.
pub(super) fn buffer_info(io: &DeviceIo, properties: &impl Properties) -> Option<BufferInfo> {
    native::buffer_info(&io.selection, properties)
}

// SAFETY: only control accesses registration/binding/rate. Native property
// queries do not mutate this owner or grant callback access to Control.
pub(super) unsafe fn alive(io: &DeviceIo, properties: &impl Properties) -> bool {
    // SAFETY: sole control worker excludes connect/disconnect until return.
    let control = unsafe { &*io.control.get() };
    if !group_ready(io, properties) {
        return false;
    }
    let connection = &control.connection;
    if native::resolve(&io.selection, properties) != Some(connection.binding) {
        return false;
    }
    control.lifecycle.registered()
        && !io.progress.invalid.load(Acquire)
        && !connection.watch_invalid.load(Acquire)
        && native::alive(connection.binding.device, properties)
        && native::admitted_format(&io.selection, connection.binding, properties).is_some_and(
            |format| format.rate == connection.rate && format.layout == connection.formats,
        )
}

fn group_ready(io: &DeviceIo, properties: &impl Properties) -> bool {
    NonNull::new(io.selection.group).is_none_or(|group| {
        // SAFETY: enclosing Native retains aggregate Arc throughout this query.
        unsafe { properties.group_ready(group) }
    })
}

// SAFETY: this query runs on the single control owner, excluding registration changes.
pub(super) unsafe fn clock_device(io: &DeviceIo, properties: &impl Properties) -> u32 {
    // SAFETY: callbacks never access the control cell.
    let control = unsafe { &*io.control.get() };
    if !control.lifecycle.registered() {
        return 0;
    }
    let device = control.connection.binding.device;
    if !io.selection.group.is_null() {
        return if group_ready(io, properties) {
            device
        } else {
            0
        };
    }
    native::clock_device(device, properties)
}
