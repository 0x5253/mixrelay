//! Direct CoreAudio/dispatch ABI. Only Objective-C message sends remain in the compiler shim.

use super::{TapError, TapSession};
use crate::control_trace::{self, Procedure};
use crate::process_tap_input::TapFormat;
use mixrelay_macos_transport::audio_buffer::AudioBufferList;
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
};

mod listeners;
mod properties;
mod readiness;
mod retirement;
mod selection;

#[cfg(test)]
pub(super) use listeners::service_restarted;
pub(super) use listeners::{add_inventory_listener, add_listener, add_service_listener};
use listeners::{format_changed, inventory_changed, service_changed};
#[cfg(test)]
pub(super) use properties::finish_property;
use properties::{object_ids, property, property_at};
#[cfg(test)]
pub(super) use retirement::{checked_queue, enqueue};
pub(super) use retirement::{prepare_cleanup, retire};
pub(super) use selection::{
    DeviceLease, ProcessIdentity, Selection, device_uses, owned_endpoint_uid,
};
use selection::{device, processes};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

impl Address {
    const fn global(selector: [u8; 4]) -> Self {
        Self {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        }
    }

    const fn output(selector: [u8; 4]) -> Self {
        Self {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"outp"),
            element: 0,
        }
    }

    const fn input(selector: [u8; 4]) -> Self {
        Self {
            selector: u32::from_be_bytes(selector),
            scope: u32::from_be_bytes(*b"inpt"),
            element: 0,
        }
    }
}

/// Native timestamp prefix and trailing SMPTE storage, matching `AudioTimeStamp`.
#[repr(C)]
#[derive(Debug, Default)]
pub(super) struct Timestamp {
    pub sample: f64,
    pub host: u64,
    pub rate_scalar: f64,
    word_clock: u64,
    smpte: [u8; 24],
    pub flags: u32,
    reserved: u32,
}

const _: () = assert!(size_of::<Timestamp>() == 64);

/// `UInt32` property extents of one `AudioObjectID` and one CF reference.
const OBJECT_ID_EXTENT: u32 = 4;
const POINTER_EXTENT: u32 = 8;
const _: () = assert!(
    size_of::<u32>() == OBJECT_ID_EXTENT as usize
        && size_of::<*const c_void>() == POINTER_EXTENT as usize
);

type IoProc = unsafe extern "C" fn(
    u32,
    *const Timestamp,
    *const AudioBufferList,
    *const Timestamp,
    *mut AudioBufferList,
    *const Timestamp,
    *mut c_void,
) -> i32;
type Listener = unsafe extern "C" fn(u32, u32, *const Address, *mut c_void) -> i32;

unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        output: *mut c_void,
    ) -> i32;
    fn AudioObjectAddPropertyListener(
        object: u32,
        address: *const Address,
        listener: Listener,
        context: *mut c_void,
    ) -> i32;
    fn AudioObjectRemovePropertyListener(
        object: u32,
        address: *const Address,
        listener: Listener,
        context: *mut c_void,
    ) -> i32;
    fn AudioDeviceCreateIOProcID(
        device: u32,
        callback: IoProc,
        context: *mut c_void,
        proc: *mut *mut c_void,
    ) -> i32;
    fn AudioDeviceDestroyIOProcID(device: u32, proc: *mut c_void) -> i32;
    fn AudioDeviceStart(device: u32, proc: *mut c_void) -> i32;
    fn AudioDeviceStop(device: u32, proc: *mut c_void) -> i32;
    fn AudioHardwareCreateProcessTap(description: *const c_void, tap: *mut u32) -> i32;
    fn AudioHardwareDestroyProcessTap(tap: u32) -> i32;
    fn AudioHardwareDestroyAggregateDevice(device: u32) -> i32;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        string: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFEqual(left: *const c_void, right: *const c_void) -> u8;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFRelease(object: *const c_void);
    fn mr_tap_description_supports_bundle_selection() -> bool;
    fn mr_tap_description_supports_device_stream_selection() -> bool;
    fn mr_tap_description_create(
        bundle: *const c_char,
        processes: *const u32,
        count: usize,
        target_device_uid: *const c_char,
        select_bundle: bool,
        mute: bool,
        system_audio: bool,
    ) -> *mut c_void;
    fn mr_tap_description_release(description: *mut c_void);
    fn mr_tap_aggregate_create(description: *mut c_void, device: *mut u32) -> i32;
}

pub(super) trait Native {
    fn device(&self, uid: &CStr) -> Result<DeviceLease, TapError>;
    fn processes(
        &self,
        bundle: &CStr,
        target_device: Option<u32>,
    ) -> Result<Vec<ProcessIdentity>, TapError>;
    fn rebuild(&self, session: &TapSession, selection: &Selection) -> Result<(), TapError>;
    fn format(&self, tap: u32) -> Result<TapFormat, TapError>;
    fn start(&self, device: u32, proc: *mut c_void) -> Result<(), TapError>;
    fn stop(&self, device: u32, proc: *mut c_void) -> Result<(), TapError>;
    /// Adopts even an error-returned identifier before returning its status.
    fn register(&self, session: &TapSession) -> Result<(), TapError>;
    fn unregister(&self, device: u32, proc: *mut c_void) -> Result<(), TapError>;
    fn remove_listener(&self, tap: u32, session: &TapSession) -> Result<(), TapError>;
    fn remove_inventory_listener(&self, session: &TapSession) -> Result<(), TapError>;
    fn remove_service_listener(&self, session: &TapSession) -> Result<(), TapError>;
    fn install_system_listeners(&self, session: &TapSession) -> Result<(), TapError>;
    fn destroy_aggregate(&self, device: u32) -> Result<(), TapError>;
    fn destroy_tap(&self, tap: u32) -> Result<(), TapError>;
    /// One temporary, unmuted observation of exactly the same selection.
    fn diagnostic(
        &self,
        selection: &Selection,
        observed_at: std::time::Instant,
    ) -> Result<super::watchdog::Diagnostic, TapError>;
    fn diagnostic_has_audio(&self, diagnostic: &super::watchdog::Diagnostic) -> bool;
    fn retire_diagnostic(&self, diagnostic: super::watchdog::Diagnostic);
}

pub(super) struct CoreAudio;

fn context(session: &TapSession) -> *mut c_void {
    std::ptr::from_ref(session).cast_mut().cast()
}

impl Native for CoreAudio {
    fn device(&self, uid: &CStr) -> Result<DeviceLease, TapError> {
        device(uid)
    }
    fn processes(
        &self,
        bundle: &CStr,
        target_device: Option<u32>,
    ) -> Result<Vec<ProcessIdentity>, TapError> {
        processes(bundle, target_device)
    }
    fn rebuild(&self, session: &TapSession, selection: &Selection) -> Result<(), TapError> {
        let description = Description::create(selection)?;
        description.tap(session)?;
        add_listener(session)?;
        description.aggregate(session)?;
        Ok(())
    }
    fn format(&self, tap: u32) -> Result<TapFormat, TapError> {
        // TapFormat's trailing alignment padding corresponds to ASBD.mReserved.
        property(tap, *b"tfmt")
    }
    fn start(&self, device: u32, proc: *mut c_void) -> Result<(), TapError> {
        control_trace::run(Procedure::StartIo, || {
            // SAFETY: control owner retains the matching native registration.
            TapError::status(unsafe { AudioDeviceStart(device, proc) })
        })
    }
    fn stop(&self, device: u32, proc: *mut c_void) -> Result<(), TapError> {
        control_trace::run(Procedure::StopIo, || {
            // SAFETY: only the serialized control owner starts/stops this registration.
            TapError::status(unsafe { AudioDeviceStop(device, proc) })
        })
    }
    fn register(&self, session: &TapSession) -> Result<(), TapError> {
        control_trace::run(Procedure::RegisterIo, || {
            let mut proc = ptr::null_mut();
            let resources = session.resources.get();
            // SAFETY: heap-pinned session is retained until unregister and callback drain.
            let status = unsafe {
                AudioDeviceCreateIOProcID(
                    resources.aggregate,
                    super::ffi::io_proc,
                    context(session),
                    &raw mut proc,
                )
            };
            session.adopt_registration(proc, status)
        })
    }
    fn unregister(&self, device: u32, proc: *mut c_void) -> Result<(), TapError> {
        control_trace::run(Procedure::UnregisterIo, || {
            // SAFETY: identifier is cleared only after this succeeds; failures retain owner.
            TapError::status(unsafe { AudioDeviceDestroyIOProcID(device, proc) })
        })
    }
    fn remove_listener(&self, tap: u32, session: &TapSession) -> Result<(), TapError> {
        control_trace::run(Procedure::RemoveFormatListener, || {
            // SAFETY: exact callback/context tuple matches registration and is still retained.
            TapError::status(unsafe {
                AudioObjectRemovePropertyListener(
                    tap,
                    &Address::global(*b"tfmt"),
                    format_changed,
                    context(session),
                )
            })
        })
    }
    fn remove_inventory_listener(&self, session: &TapSession) -> Result<(), TapError> {
        control_trace::run(Procedure::RemoveInventoryListener, || {
            // SAFETY: the listener/context pair is identical to registration and the
            // session stays owned until removal and active-listener drain complete.
            TapError::status(unsafe {
                AudioObjectRemovePropertyListener(
                    1,
                    &Address::global(*b"prs#"),
                    inventory_changed,
                    context(session),
                )
            })
        })
    }
    fn destroy_aggregate(&self, device: u32) -> Result<(), TapError> {
        control_trace::run(Procedure::DestroyAggregate, || {
            // SAFETY: control owner removed IOProc before destroying its aggregate.
            TapError::status(unsafe { AudioHardwareDestroyAggregateDevice(device) })
        })
    }
    fn remove_service_listener(&self, session: &TapSession) -> Result<(), TapError> {
        control_trace::run(Procedure::RemoveServiceListener, || {
            // SAFETY: exact registered pair, retained until removal and active-listener drain.
            TapError::status(unsafe {
                AudioObjectRemovePropertyListener(
                    1,
                    &Address::global(*b"srst"),
                    service_changed,
                    context(session),
                )
            })
        })
    }
    fn install_system_listeners(&self, session: &TapSession) -> Result<(), TapError> {
        add_service_listener(session)?;
        if session
            .selection
            .borrow()
            .as_ref()
            .is_some_and(|selection| selection.fallback)
        {
            add_inventory_listener(session)?;
        }
        Ok(())
    }
    fn destroy_tap(&self, tap: u32) -> Result<(), TapError> {
        control_trace::run(Procedure::DestroyTap, || {
            // SAFETY: aggregate and listener are gone before their underlying tap.
            TapError::status(unsafe { AudioHardwareDestroyProcessTap(tap) })
        })
    }
    fn diagnostic(
        &self,
        selection: &Selection,
        _observed_at: std::time::Instant,
    ) -> Result<super::watchdog::Diagnostic, TapError> {
        let preparation_started = std::time::Instant::now();
        let current = selection.processes(self)?;
        if selection.fallback && current != selection.selected {
            return Err(TapError::code(9));
        }
        if let (Some(uid), Some(expected)) = (&selection.target_device_uid, selection.target_device)
            && self.device(uid)? != expected
        {
            return Err(TapError::code(9));
        }
        if selection.exclusions.is_none()
            && !current.iter().any(|process| {
                property::<u32>(process.object, *b"piro").is_ok_and(|running| running != 0)
            })
        {
            return Err(TapError::code(3));
        }
        super::ffi::create_diagnostic(selection).map(|session| super::watchdog::Diagnostic {
            session,
            preparation_started,
            preparation_time: preparation_started.elapsed(),
        })
    }
    fn diagnostic_has_audio(&self, diagnostic: &super::watchdog::Diagnostic) -> bool {
        // SAFETY: the control watchdog retains this diagnostic until retirement.
        unsafe { &*diagnostic.session }
            .audible_callbacks
            .load(std::sync::atomic::Ordering::Relaxed)
            != 0
    }
    fn retire_diagnostic(&self, diagnostic: super::watchdog::Diagnostic) {
        // SAFETY: the control watchdog transfers its unique ownership exactly once.
        unsafe { super::ffi::mr_process_tap_destroy(diagnostic.session) };
    }
}

pub(super) struct Description(*mut c_void);

impl Description {
    pub(super) fn create(selection: &Selection) -> Result<Self, TapError> {
        control_trace::run(Procedure::CreateDescription, || {
            let processes: Vec<u32> = selection.selected.iter().map(|id| id.object).collect();
            // SAFETY: create validated UTF-8 input; process slice remains readable for this call.
            let pointer = unsafe {
                mr_tap_description_create(
                    selection.bundle.as_ptr(),
                    processes.as_ptr(),
                    processes.len(),
                    selection
                        .target_device_uid
                        .as_ref()
                        .map_or(ptr::null(), |uid| uid.as_ptr()),
                    !selection.fallback,
                    selection.mute,
                    selection.exclusions.is_some(),
                )
            };
            if pointer.is_null() {
                Err(TapError::code(6))
            } else {
                Ok(Self(pointer))
            }
        })
    }

    pub(super) fn tap(&self, session: &TapSession) -> Result<(), TapError> {
        control_trace::run(Procedure::CreateTap, || {
            let mut resources = session.resources.get();
            // SAFETY: retained CATapDescription and exclusive local result storage.
            let result = unsafe { AudioHardwareCreateProcessTap(self.0, &raw mut resources.tap) };
            session.resources.set(resources);
            TapError::status(result)
        })
    }

    pub(super) fn aggregate(&self, session: &TapSession) -> Result<(), TapError> {
        let mut resources = session.resources.get();
        control_trace::run(Procedure::CreateAggregate, || {
            // SAFETY: shim constructs only Objective-C dictionary; Rust retains returned owner on failure.
            let result = unsafe { mr_tap_aggregate_create(self.0, &raw mut resources.aggregate) };
            session.resources.set(resources);
            TapError::status(result)
        })?;
        control_trace::run(Procedure::AggregateReady, || {
            readiness::wait(resources.tap, resources.aggregate, &CoreAudio)
        })
    }
}

impl Drop for Description {
    fn drop(&mut self) {
        // SAFETY: helper consumes the +1 retained Objective-C description exactly once.
        unsafe { mr_tap_description_release(self.0) };
    }
}
