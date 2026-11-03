//! The `CoreAudio` operations of device preparation and each owner's control worker.
//! Production forwards every call to the transport ABI. Hermetic tests substitute a
//! simulated device behind the same contract; no call here runs on realtime threads.
use super::{
    aggregate::Aggregate,
    buffer_control::BufferInfo,
    ffi,
    group_plan::{GroupPlan, MemberInfo},
    native::Native,
    spec::Spec,
};
use mixrelay_audio_io::{AudioDeviceDescriptor, DeviceSelector, HostAudioBackendError};
use mixrelay_macos_transport::aggregate_group::NativeBinding;
use std::{
    ffi::{CStr, c_void},
    ptr::NonNull,
    sync::Arc,
};

pub(super) trait Host: Send + Sync {
    fn set_permission_waiting(&self, waiting: bool);
    fn permission_waiting(&self) -> bool;
    /// Consent for this process, without presenting a new privacy prompt.
    fn input_permission(&self) -> super::permission::InputPermission;
    /// Presents first-use consent without blocking the native worker.
    fn request_input_permission(&self);
    /// Waits one bounded control-side interval and observes the authorization decision.
    fn poll_input_permission(&self) -> super::permission::InputPermission;
    /// Present devices for these selections; an absent selection is omitted, not an error.
    fn inventory(
        &self,
        selectors: &[&DeviceSelector],
    ) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError>;
    /// Current facts of one prospective private-group member, or None when unavailable.
    fn member_info(&self, uid: &CStr) -> Option<MemberInfo>;
    /// Creates a private group for `plan`. A returned owner transfers even with a nonzero
    /// status: 1 rejects the plan, 2 reports native failure, 0 still requires readiness.
    fn create_group(&self, plan: &GroupPlan) -> (Option<NonNull<c_void>>, i32);
    /// Whether the group's published identity, geometry and clock policy match its plan.
    fn group_ready(&self, group: NonNull<c_void>) -> bool;
    /// Destroys a retired group once no registration uses it. False retains it for retry.
    fn destroy_group(&self, group: NonNull<c_void>) -> bool;
    /// Allocates one disconnected native owner for `spec`'s native `uid`, in `group` if any.
    /// None for invalid geometry or allocation failure.
    fn create(
        &self,
        spec: &Spec,
        uid: &CStr,
        rate: u32,
        group: Option<&Aggregate>,
    ) -> Option<NonNull<c_void>>;
    /// The owner's resolved buffer facts. False while its device cannot be resolved.
    fn buffer_info(&self, native: &Native, info: &mut BufferInfo) -> bool;
    /// One device's current client buffer, or zero when unavailable.
    fn buffer_size(&self, device: u32) -> u32;
    /// Requests a client buffer; success is not acceptance until read back.
    fn write_buffer(&self, device: u32, frames: u32) -> bool;
    /// Registers and starts the owner's callback: zero, or a native/preflight status.
    fn connect(&self, native: &Native, expected: u32) -> i32;
    /// Stops and unregisters. False retains the registration for another attempt.
    fn disconnect(&self, native: &Native) -> bool;
    /// Unregisters if needed and releases the owner. False retains it for retry; true
    /// ends every use of its pointer.
    fn destroy(&self, native: &Native) -> bool;
    /// The owner's current all-Float32 projection for in-place rendering, from its
    /// immutable selection: the resolved device and its channel slice.
    fn direct_binding(&self, native: &Native) -> Option<NativeBinding>;
    /// Whether the connected device, its format and selection are still current.
    fn alive(&self, native: &Native) -> bool;
    /// The connection's proven clock object, or zero when unknown.
    fn clock_device(&self, native: &Native) -> u32;
    /// Completed callbacks: the connection's progress evidence.
    fn ticks(&self, native: &Native) -> u64;
}

/// The process's `CoreAudio` HAL through the transport ABI.
#[derive(Default)]
pub(super) struct CoreAudio {
    permission_waiting: std::sync::atomic::AtomicBool,
    credit_signal: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl CoreAudio {
    pub(super) fn with_credit_signal(credit_signal: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        Self {
            permission_waiting: std::sync::atomic::AtomicBool::new(false),
            credit_signal,
        }
    }
}

impl Host for CoreAudio {
    fn set_permission_waiting(&self, waiting: bool) {
        // This flag publishes no other data; status readers only need the current reason.
        self.permission_waiting
            .store(waiting, std::sync::atomic::Ordering::Relaxed);
    }
    fn permission_waiting(&self) -> bool {
        self.permission_waiting
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    fn input_permission(&self) -> super::permission::InputPermission {
        super::permission::current()
    }
    fn request_input_permission(&self) {
        super::permission::request();
    }
    fn poll_input_permission(&self) -> super::permission::InputPermission {
        std::thread::sleep(super::permission::CONSENT_POLL_INTERVAL);
        self.input_permission()
    }
    fn inventory(
        &self,
        selectors: &[&DeviceSelector],
    ) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
        mixrelay_macos_audio_io::selected_devices(selectors.iter().copied())
    }
    fn member_info(&self, uid: &CStr) -> Option<MemberInfo> {
        let mut info = MemberInfo::default();
        // SAFETY: the native call borrows a NUL-terminated UID and writes
        // exactly one repr(C) snapshot; it retains neither address.
        unsafe { ffi::mr_aggregate_member_info(uid.as_ptr(), &raw mut info) }.then_some(info)
    }
    fn create_group(&self, plan: &GroupPlan) -> (Option<NonNull<c_void>>, i32) {
        let names = plan
            .names
            .iter()
            .map(|uid| uid.as_ptr())
            .collect::<Vec<_>>();
        let (Ok(count), true) = (u32::try_from(names.len()), names.len() == plan.facts.len())
        else {
            return (None, 1);
        };
        let mut status = 0;
        // SAFETY: both arrays hold `count` initialized entries with terminated UIDs, live and
        // immutable for the call; `status` is exclusive. Any returned owner transfers to us.
        let group = unsafe {
            ffi::mr_aggregate_group_create(
                names.as_ptr(),
                plan.facts.as_ptr(),
                count,
                0,
                &raw mut status,
            )
        };
        (NonNull::new(group), status)
    }
    fn group_ready(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: the Aggregate retains this owner, excluding retirement during the query.
        unsafe { ffi::mr_aggregate_group_ready(group.as_ptr()) }
    }
    fn destroy_group(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: called only from retirement after every registration released the group;
        // failure leaves the owner retained for the next attempt.
        unsafe { ffi::mr_aggregate_group_try_destroy(group.as_ptr()) }
    }
    fn create(
        &self,
        spec: &Spec,
        uid: &CStr,
        rate: u32,
        group: Option<&Aggregate>,
    ) -> Option<NonNull<c_void>> {
        // SAFETY: the UID is terminated and borrowed for this call; a non-null group is the
        // caller's live aggregate, retained by the new owner until its acknowledged destruction.
        let pointer = NonNull::new(unsafe {
            ffi::mr_device_io_create_group(
                group.map_or(std::ptr::null_mut(), |group| group.pointer.as_ptr()),
                uid.as_ptr(),
                spec.input,
                u32::from(spec.channels),
                rate,
            )
        })?;
        if let Some(signal) = &self.credit_signal {
            // SAFETY: this newly created owner is disconnected and exclusively held here;
            // no callback can borrow its DeviceIo until connect publishes the registration.
            unsafe {
                pointer
                    .cast::<mixrelay_macos_transport::device_io::DeviceIo>()
                    .as_mut()
            }
            .set_credit_signal(Arc::clone(signal));
        }
        Some(pointer)
    }
    fn buffer_info(&self, native: &Native, info: &mut BufferInfo) -> bool {
        // SAFETY: Native retains its owner; `info` is exclusive storage for one snapshot.
        unsafe { ffi::mr_device_io_buffer_info(native.pointer.as_ptr(), info) }
    }
    fn buffer_size(&self, device: u32) -> u32 {
        // SAFETY: a scalar property read of a device identity; no storage is borrowed.
        unsafe { ffi::mr_device_buffer_size(device) }
    }
    fn write_buffer(&self, device: u32, frames: u32) -> bool {
        // SAFETY: a synchronous scalar property write; no storage is retained.
        unsafe { ffi::mr_device_buffer_write(device, frames) }
    }
    fn connect(&self, native: &Native, expected: u32) -> i32 {
        // SAFETY: Native retains its owner and its sole control worker serializes registration.
        unsafe { ffi::mr_device_io_connect_status(native.pointer.as_ptr(), expected) }
    }
    fn disconnect(&self, native: &Native) -> bool {
        // SAFETY: the sole control worker unregisters; failure keeps the owner allocated.
        unsafe { ffi::mr_device_io_disconnect(native.pointer.as_ptr()) }
    }
    fn destroy(&self, native: &Native) -> bool {
        // SAFETY: called only from Native's drop, its sole remaining owner, after the
        // worker and graph roles stopped; failure keeps the allocation for retry.
        unsafe { ffi::mr_device_io_destroy(native.pointer.as_ptr()) }
    }
    fn direct_binding(&self, native: &Native) -> Option<NativeBinding> {
        let mut binding = NativeBinding::default();
        // SAFETY: Native retains its owner; the outputs are three disjoint local scalars.
        unsafe {
            ffi::mr_device_io_direct_binding(
                native.pointer.as_ptr(),
                &raw mut binding.device,
                &raw mut binding.offset,
                &raw mut binding.total,
            )
        }
        .then_some(binding)
    }
    fn alive(&self, native: &Native) -> bool {
        // SAFETY: a control-only query of the retained owner, serialized with registration.
        unsafe { ffi::mr_device_io_alive(native.pointer.as_ptr()) }
    }
    fn clock_device(&self, native: &Native) -> u32 {
        // SAFETY: a control-only query of the retained owner, serialized with registration.
        unsafe { ffi::mr_device_io_clock_device(native.pointer.as_ptr()) }
    }
    fn ticks(&self, native: &Native) -> u64 {
        // SAFETY: an atomic progress read of the retained owner.
        unsafe { ffi::mr_device_io_ticks(native.pointer.as_ptr()) }
    }
}
