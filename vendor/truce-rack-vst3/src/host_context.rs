//! Retained VST3 host context and bounded component-handler notifications.
//!
//! `IComponentHandler` callbacks are accepted from any thread: they only enqueue into the
//! atomic-only inbox and never call the controller. That guarantee does not apply to UI/control
//! COM services: [`message`] implements `IMessage` and `IAttributeList` operations which may
//! allocate and lock, and are not real-time callbacks. `service_main_thread` remains
//! creator-thread-only per the core contract. The two-bank writer registration closes each bank
//! before service reuses it.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};
use truce_rack_core::error::{Error, Result};
use vst3::{
    Class, ComPtr, ComWrapper,
    Steinberg::{
        FUnknown, IPluginBase,
        Vst::{
            IComponentHandler, IComponentHandlerTrait, IEditController, IEditControllerTrait,
            IHostApplication, IHostApplicationTrait, RestartFlags_, String128,
        },
        kInvalidArgument, kNoInterface, kResultOk,
    },
};

mod message;

const EDIT_CAPACITY: usize = 4096;
const MAX_CALLBACK_RETRIES: usize = 4;

pub(super) struct RestartRequests {
    pub(super) latency: bool,
    pub(super) parameter_titles: bool,
}

pub(super) fn parse_restart_flags(flags: u32) -> Result<RestartRequests> {
    let latency = RestartFlags_::kLatencyChanged as u32;
    let parameter_titles = RestartFlags_::kParamTitlesChanged as u32;
    let parameter_values = RestartFlags_::kParamValuesChanged as u32;
    if flags & !(latency | parameter_titles | parameter_values) != 0 {
        return Err(callback_error("unsupported component restart flags"));
    }
    Ok(RestartRequests {
        latency: flags & latency != 0,
        parameter_titles: flags & parameter_titles != 0,
    })
}

struct EditSlot {
    id_plus_one: AtomicU64,
    value_bits: AtomicU64,
}

impl EditSlot {
    fn new() -> Self {
        Self {
            id_plus_one: AtomicU64::new(0),
            value_bits: AtomicU64::new(0.0_f64.to_bits()),
        }
    }
}

struct EditBank {
    slots: Vec<EditSlot>,
    count: AtomicUsize,
    writers: AtomicUsize,
    closed: AtomicBool,
}

impl EditBank {
    fn new() -> Self {
        Self {
            slots: (0..EDIT_CAPACITY).map(|_| EditSlot::new()).collect(),
            count: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn insert(&self, id: u32, value: f64) -> bool {
        let encoded = u64::from(id) + 1;
        for slot in &self.slots {
            let present = slot.id_plus_one.load(Ordering::Acquire);
            if present == encoded {
                slot.value_bits.store(value.to_bits(), Ordering::Release);
                return true;
            }
            if present == 0 {
                match slot.id_plus_one.compare_exchange(
                    0,
                    encoded,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        slot.value_bits.store(value.to_bits(), Ordering::Release);
                        self.count.fetch_add(1, Ordering::Release);
                        return true;
                    }
                    Err(actual) if actual == encoded => {
                        slot.value_bits.store(value.to_bits(), Ordering::Release);
                        return true;
                    }
                    Err(_) => {}
                }
            }
        }
        false
    }

    fn drain(&self, mut apply: impl FnMut(u32, f64) -> bool) -> bool {
        if self.count.load(Ordering::Acquire) == 0 {
            self.closed.store(false, Ordering::SeqCst);
            return true;
        }
        for slot in &self.slots {
            let encoded = slot.id_plus_one.load(Ordering::Acquire);
            if encoded == 0 {
                continue;
            }
            let Ok(id) = u32::try_from(encoded - 1) else {
                return false;
            };
            let value = f64::from_bits(slot.value_bits.load(Ordering::Acquire));
            if !apply(id, value) {
                return false;
            }
        }
        for slot in &self.slots {
            slot.id_plus_one.store(0, Ordering::Release);
        }
        self.count.store(0, Ordering::Release);
        self.closed.store(false, Ordering::SeqCst);
        true
    }
}

/// Two fixed banks let service rotate without losing edits arriving during a tick.
///
/// Bank membership spans separate `active`, `closed`, and `writers` atomics. Those protocol
/// operations are sequentially consistent so registration, rotation, closure, and the zero-writer
/// check share one total order: writers validated before a rotation are counted before the drain
/// check, while writers registering after it fail validation before touching slot payloads.
struct EditInbox {
    banks: [EditBank; 2],
    active: AtomicUsize,
}

impl EditInbox {
    fn new() -> Self {
        Self {
            banks: [EditBank::new(), EditBank::new()],
            active: AtomicUsize::new(0),
        }
    }

    fn push(&self, id: u32, value: f64) -> bool {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return false;
        }
        for _ in 0..MAX_CALLBACK_RETRIES {
            let index = self.active.load(Ordering::SeqCst);
            let bank = &self.banks[index];
            bank.writers.fetch_add(1, Ordering::SeqCst);
            if bank.closed.load(Ordering::SeqCst) || self.active.load(Ordering::SeqCst) != index {
                bank.writers.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let inserted = bank.insert(id, value);
            bank.writers.fetch_sub(1, Ordering::SeqCst);
            return inserted;
        }
        false
    }

    fn has_pending(&self) -> bool {
        self.banks.iter().any(|bank| {
            bank.count.load(Ordering::Acquire) != 0 || bank.writers.load(Ordering::SeqCst) != 0
        })
    }

    fn drain_inactive(&self, apply: &mut impl FnMut(u32, f64) -> bool) -> Result<()> {
        let active = self.active.load(Ordering::SeqCst);
        let inactive = &self.banks[1 - active];
        if !inactive.closed.load(Ordering::SeqCst) || inactive.writers.load(Ordering::SeqCst) != 0 {
            return Ok(());
        }
        if !inactive.drain(apply) {
            return Err(callback_error("editor edit could not be delivered"));
        }
        Ok(())
    }

    fn rotate(&self, apply: &mut impl FnMut(u32, f64) -> bool) -> Result<()> {
        let current = self.active.load(Ordering::SeqCst);
        let next = 1 - current;
        let target = &self.banks[next];
        if target.closed.load(Ordering::SeqCst)
            || target.writers.load(Ordering::SeqCst) != 0
            || target.count.load(Ordering::Acquire) != 0
        {
            return Ok(());
        }
        self.active.store(next, Ordering::SeqCst);
        let previous = &self.banks[current];
        previous.closed.store(true, Ordering::SeqCst);
        if previous.writers.load(Ordering::SeqCst) == 0 && !previous.drain(apply) {
            return Err(callback_error("editor edit could not be delivered"));
        }
        Ok(())
    }

    fn service(&self, mut apply: impl FnMut(u32, f64) -> bool) -> Result<()> {
        self.drain_inactive(&mut apply)?;
        self.rotate(&mut apply)
    }
}

struct CallbackState {
    edits: EditInbox,
    restart_flags: AtomicU32,
    failed: AtomicBool,
    latency_revision: AtomicU64,
    latency_snapshot_revision: AtomicU64,
}

impl CallbackState {
    fn new() -> Self {
        Self {
            edits: EditInbox::new(),
            restart_flags: AtomicU32::new(0),
            failed: AtomicBool::new(false),
            latency_revision: AtomicU64::new(1),
            latency_snapshot_revision: AtomicU64::new(0),
        }
    }

    fn invalidate_latency(&self) {
        self.latency_revision.fetch_add(1, Ordering::AcqRel);
    }

    fn perform_edit(&self, id: u32, value: f64) -> i32 {
        if self.edits.push(id, value) {
            self.invalidate_latency();
            kResultOk
        } else {
            self.failed.store(true, Ordering::Release);
            self.invalidate_latency();
            kInvalidArgument
        }
    }

    fn restart(&self, flags: i32) -> i32 {
        let Ok(flags) = u32::try_from(flags) else {
            self.failed.store(true, Ordering::Release);
            self.invalidate_latency();
            return kInvalidArgument;
        };
        if flags != 0 {
            self.invalidate_latency();
        }
        self.restart_flags.fetch_or(flags, Ordering::AcqRel);
        kResultOk
    }
}

struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> i32 {
        if name.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the required output pointer is non-null and VST3 defines String128 as 128 UTF-16 units.
        unsafe {
            *name = [0; 128];
            for (destination, source) in (*name).iter_mut().zip("MixRelay".encode_utf16()) {
                *destination = source;
            }
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut vst3::Steinberg::TUID,
        iid: *mut vst3::Steinberg::TUID,
        object: *mut *mut std::ffi::c_void,
    ) -> i32 {
        if object.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: non-null output pointer is writable by the COM caller contract.
        unsafe { *object = std::ptr::null_mut() };
        if cid.is_null() || iid.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: VST3 supplies pointers to readable, 16-byte TUID values.
        let (class_id, interface_id) = unsafe { (*cid, *iid) };
        let Some(object_ptr) = message::create_instance(&class_id, &interface_id) else {
            return kNoInterface;
        };
        // SAFETY: on success, transfer the initial COM reference to the caller.
        unsafe { *object = object_ptr };
        kResultOk
    }
}

struct ComponentHandler {
    state: Arc<CallbackState>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: u32) -> i32 {
        // This host does not publish automation gestures; value delivery is handled by performEdit.
        kResultOk
    }

    unsafe fn performEdit(&self, id: u32, value: f64) -> i32 {
        self.state.perform_edit(id, value)
    }

    unsafe fn endEdit(&self, _id: u32) -> i32 {
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: i32) -> i32 {
        self.state.restart(flags)
    }
}

/// Owned host objects stay alive until after component/controller termination and COM release.
pub(super) struct HostServices {
    application: ComWrapper<HostApplication>,
    handler: ComWrapper<ComponentHandler>,
    state: Arc<CallbackState>,
    creator_thread: std::thread::ThreadId,
}

impl HostServices {
    pub(super) fn new() -> Self {
        let state = Arc::new(CallbackState::new());
        Self {
            application: ComWrapper::new(HostApplication),
            handler: ComWrapper::new(ComponentHandler {
                state: Arc::clone(&state),
            }),
            state,
            creator_thread: std::thread::current().id(),
        }
    }

    pub(super) fn ensure_creator_thread(&self) -> Result<()> {
        if std::thread::current().id() != self.creator_thread {
            return Err(callback_error(
                "main-thread VST3 API called from a non-creator thread",
            ));
        }
        Ok(())
    }

    pub(super) fn context_ptr(&self) -> *mut FUnknown {
        self.application
            .as_com_ref::<IHostApplication>()
            .expect("host application interface")
            .as_ptr()
            .cast::<FUnknown>()
    }

    pub(super) fn attach_handler(&self, controller: &ComPtr<IEditController>) -> Result<()> {
        let handler = self
            .handler
            .as_com_ref::<IComponentHandler>()
            .expect("component handler interface");
        // SAFETY: the retained handler outlives controller termination; VST3 receives a valid
        // interface pointer synchronously on the instance's creator thread.
        if unsafe { controller.setComponentHandler(handler.as_ptr()) } != kResultOk {
            return Err(callback_error(
                "IEditController::setComponentHandler failed",
            ));
        }
        Ok(())
    }

    pub(super) fn initialize_separate_controller(
        &self,
        base: &ComPtr<IPluginBase>,
        controller: &ComPtr<IEditController>,
    ) -> Result<crate::lifecycle::InitializedPlugin> {
        let guard = crate::lifecycle::InitializedPlugin::initialize(base, self.context_ptr())?;
        self.attach_handler(controller)?;
        Ok(guard)
    }

    pub(super) fn take_restart_flags(&self) -> u32 {
        self.state.restart_flags.swap(0, Ordering::AcqRel)
    }

    pub(super) fn invalidate_latency(&self) {
        self.state.invalidate_latency();
    }

    pub(super) fn latency_revision(&self) -> u64 {
        self.state.latency_revision.load(Ordering::Acquire)
    }

    pub(super) fn publish_latency_revision(&self, revision: u64) {
        self.state
            .latency_snapshot_revision
            .store(revision, Ordering::Release);
    }

    pub(super) fn latency_snapshot_valid(&self) -> bool {
        !self.callbacks_failed()
            && self.state.latency_revision.load(Ordering::Acquire)
                == self.state.latency_snapshot_revision.load(Ordering::Acquire)
    }

    pub(super) fn service_edits(&self, mut apply: impl FnMut(u32, f64) -> bool) -> Result<()> {
        if self.state.failed.load(Ordering::Acquire) {
            return Err(callback_error(
                "VST3 host callback rejected invalid or excess data",
            ));
        }
        self.state.edits.service(&mut apply)?;
        if self.callbacks_failed() {
            return Err(callback_error("VST3 host callback failed during service"));
        }
        Ok(())
    }

    pub(super) fn has_pending_edits(&self) -> bool {
        self.state.edits.has_pending() || self.callbacks_failed()
    }

    pub(super) fn callbacks_failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
    }

    pub(super) fn fail_closed(&self) {
        self.state.failed.store(true, Ordering::Release);
        self.invalidate_latency();
    }
}

fn callback_error(reason: &str) -> Error {
    Error::Other(format!("VST3 host callback: {reason}"))
}

#[cfg(test)]
mod tests;
