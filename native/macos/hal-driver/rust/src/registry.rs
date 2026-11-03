//! Device inventory and bounded pins for device identities and metadata.
mod clients;
pub(crate) use clients::ClientIdentity;
mod configuration;

use std::ffi::c_void;
use std::ops::Deref;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};

use crate::abi::{
    ALIVE, CAPACITY, Geometry, HostOps, INVALID, LIST, MAX_DEVICES, NAME, NATIVE_ERROR, NOT_FOUND,
    NativeOps, OK, OUTPUT_MUTE, OUTPUT_VOLUME, RUNNING, STREAM_ACTIVE, Snapshot, Timestamp,
};
use crate::device::{Attachment, Device, Pending, Prepared, lock};

// The control bridge ticks every 100 ms. A host proxy ACK does not guarantee
// Perform will arrive; retain the owned attachment and retry after five seconds.
pub(crate) const CONFIG_ACK_TIMEOUT_TICKS: u32 = 50;

struct Slot {
    pointer: AtomicPtr<Device>,
    readers: AtomicUsize,
}

enum AttachmentChange {
    Queued,
    Renamed,
    Unchanged,
}

#[derive(Clone, Copy)]
pub(crate) struct IoOperation {
    pub device: u32,
    pub stream: u32,
    pub render: bool,
    pub client: u32,
    pub frame: u64,
    pub samples: *mut c_void,
    pub frames: u32,
}

pub(crate) struct Registry {
    pub(crate) control_signal: std::sync::OnceLock<crate::control_signal::ControlSignal>,
    pub(crate) native: NativeOps,
    pub(crate) client_identity_gate: Mutex<()>,
    pub(crate) transport: crate::transport::Transport,
    inspection: crate::transport::ProcessInspection,
    host: HostOps,
    /// Orders every settled configuration request for the Driver service.
    settlements: AtomicU64,
    /// Random identity of this HAL instance. Settlement sequences restart with it, so
    /// the Driver service compares sequences only within one incarnation.
    pub(crate) incarnation: u64,
    /// Settled requests not yet superseded by `SETTLED_RETAINED` newer ones.
    settled: Mutex<std::collections::VecDeque<crate::device::Settled>>,
    // Object generation is shared across slots. Slots are not reused until a
    // retired device has no clients and all reader pins have been released.
    generation: Mutex<u32>,
    slots: [Slot; MAX_DEVICES],
    #[cfg(target_os = "macos")]
    pub(crate) inventory: Mutex<crate::inventory::InventoryState>,
}

pub(crate) struct Pin<'a> {
    slot: &'a Slot,
    device: *mut Device,
}

impl Deref for Pin<'_> {
    type Target = Device;
    fn deref(&self) -> &Device {
        unsafe { &*self.device }
    }
}

impl Drop for Pin<'_> {
    fn drop(&mut self) {
        self.slot.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

unsafe extern "C" {
    fn arc4random_buf(buffer: *mut std::ffi::c_void, length: usize);
}

/// A nonzero random identity drawn once per HAL load, on control.
fn incarnation() -> u64 {
    let mut bytes = [0_u8; 8];
    // SAFETY: the buffer owns all eight bytes for this call.
    unsafe { arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
    u64::from_ne_bytes(bytes).max(1)
}

impl Registry {
    pub(crate) fn new(native: NativeOps, host: HostOps) -> Self {
        Self::with_inspection(
            native,
            host,
            crate::transport::ProcessInspection::SYSTEM,
            mixrelay_time::StdClock::new(),
        )
    }
    pub(crate) fn with_inspection(
        native: NativeOps,
        host: HostOps,
        inspection: crate::transport::ProcessInspection,
        clock: impl mixrelay_time::MonotonicClock + 'static,
    ) -> Self {
        Self {
            control_signal: std::sync::OnceLock::new(),
            native,
            transport: crate::transport::Transport::with_inspection(inspection, clock),
            inspection,
            client_identity_gate: Mutex::new(()),
            host,
            settlements: AtomicU64::new(0),
            incarnation: incarnation(),
            settled: Mutex::new(std::collections::VecDeque::with_capacity(
                configuration::SETTLED_RETAINED,
            )),
            generation: Mutex::new(0),
            slots: std::array::from_fn(|_| Slot {
                pointer: AtomicPtr::default(),
                readers: AtomicUsize::new(0),
            }),
            #[cfg(target_os = "macos")]
            inventory: Mutex::new(crate::inventory::InventoryState::default()),
        }
    }

    fn pin_slot(&self, index: usize) -> Option<Pin<'_>> {
        let slot = &self.slots[index];
        slot.readers.fetch_add(1, Ordering::SeqCst);
        let device = slot.pointer.load(Ordering::SeqCst);
        if device.is_null() {
            slot.readers.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Pin { slot, device })
    }

    pub(crate) fn pin(&self, object: u32) -> Option<Pin<'_>> {
        let offset = object.checked_sub(16)?;
        let index = (offset / 16) as usize % MAX_DEVICES;
        let pin = self.pin_slot(index)?;
        // Device generations reserve a stride of 16: streams use +1/+2 and
        // native output volume/mute controls use +3/+4 without slot collisions.
        (object >= pin.id && object <= pin.id + 4).then_some(pin)
    }

    fn endpoint(&self, endpoint: [u8; 16]) -> Option<Pin<'_>> {
        (0..MAX_DEVICES)
            .filter_map(|index| self.pin_slot(index))
            .find(|device| device.endpoint == endpoint && device.alive.load(Ordering::Acquire))
    }

    pub(crate) fn upsert(
        &self,
        endpoint: [u8; 16],
        name: &str,
        rate: u32,
        transport: (OwnedFd, usize),
    ) -> Result<u32, i32> {
        let (descriptor, size) = transport;
        if name.is_empty()
            || name.len() >= 128
            || name.contains('\0')
            || !crate::formats::SAMPLE_RATES.contains(&rate)
        {
            return Err(INVALID);
        }
        let mut geometry = Geometry::default();
        if !unsafe { (self.native.inspect)(descriptor.as_raw_fd(), size, &raw mut geometry) }
            || !geometry.valid()
            || geometry.endpoint_id != endpoint
        {
            return Err(INVALID);
        }
        let attachment = Attachment {
            descriptor,
            size,
            geometry,
            prepared: None,
        };
        let mut generation = lock(&self.generation);
        if let Some(device) = self.endpoint(endpoint) {
            let id = device.id;
            let change = Self::queue_attachment(&device, name, attachment)?;
            drop(device);
            drop(generation);
            match change {
                AttachmentChange::Queued => self.request(id),
                AttachmentChange::Renamed => self.host.changed(id, NAME),
                AttachmentChange::Unchanged => {}
            }
            return Ok(id);
        }
        let index = self
            .slots
            .iter()
            .position(|slot| slot.pointer.load(Ordering::SeqCst).is_null())
            .ok_or(CAPACITY)?;
        let serial = generation
            .checked_mul(u32::try_from(MAX_DEVICES).map_err(|_| CAPACITY)?)
            .ok_or(CAPACITY)?;
        let id = serial
            .checked_add(u32::try_from(index).map_err(|_| CAPACITY)?)
            .and_then(|id| id.checked_mul(16))
            .and_then(|id| id.checked_add(16))
            .ok_or(CAPACITY)?;
        let next_generation = generation.checked_add(1).ok_or(CAPACITY)?;
        let runtime = unsafe {
            (self.native.create)(
                &raw const geometry,
                rate,
                attachment.descriptor.as_raw_fd(),
                size,
            )
        };
        if runtime.is_null() {
            return Err(NATIVE_ERROR);
        }
        let device = if let Ok(device) =
            Device::new(id, geometry, name.into(), rate, (self.native, runtime))
        {
            Box::new(device)
        } else {
            // SAFETY: the unpublished runtime has no callback readers.
            unsafe { (self.native.destroy)(runtime) };
            return Err(CAPACITY);
        };
        self.slots[index]
            .pointer
            .store(Box::into_raw(device), Ordering::SeqCst);
        *generation = next_generation;
        drop(generation);
        self.host.changed(1, LIST);
        Ok(id)
    }

    fn queue_attachment(
        device: &Device,
        name: &str,
        mut attachment: Attachment,
    ) -> Result<AttachmentChange, i32> {
        let mut control = lock(&device.control);
        if control.pending.is_none()
            && unsafe {
                (device.native.same_transport)(
                    device.runtime.load(Ordering::Acquire),
                    attachment.descriptor.as_raw_fd(),
                    attachment.size,
                )
            }
        {
            control.attachment_failed = false;
            if control.name == name {
                return Ok(AttachmentChange::Unchanged);
            }
            control.name = name.into();
            return Ok(AttachmentChange::Renamed);
        }
        let generation = control.next_generation;
        control.next_generation = generation.checked_add(1).ok_or(CAPACITY)?;
        // Map and validate the endpoint before coreaudiod is asked to perform the change,
        // so Perform only swaps it in. Client queues are mapped at each client's IO start.
        // SAFETY: the attachment owns its descriptor and validated geometry.
        attachment.prepared = Prepared::new(
            unsafe {
                (device.native.prepare)(
                    &raw const attachment.geometry,
                    attachment.descriptor.as_raw_fd(),
                    attachment.size,
                )
            },
            device.native,
        );
        // Reconnection must not overwrite a DAW-owned nominal rate with stale
        // Driver service inventory metadata. A pending explicit rate request is preserved too.
        let rate = control.pending.as_ref().map_or_else(
            || device.rate.load(Ordering::Acquire),
            |pending| pending.rate,
        );
        let formats = control.pending.as_ref().map_or_else(
            || {
                [
                    device.capture_format.format(),
                    device.render_format.format(),
                ]
            },
            |pending| pending.formats,
        );
        control.pending = Some(Pending {
            generation,
            requested: false,
            retry_after: 0,
            failures: 0,
            attachment: Some(attachment),
            name: name.into(),
            rate,
            formats,
        });
        control.attachment_failed = false;
        Ok(AttachmentChange::Queued)
    }

    pub(crate) fn remove(&self, endpoint: [u8; 16]) -> i32 {
        let guard = lock(&self.generation);
        let Some(device) = self.endpoint(endpoint) else {
            return NOT_FOUND;
        };
        let id = device.id;
        let mut control = lock(&device.control);
        let frozen = device.freeze();
        device.alive.store(false, Ordering::Release);
        // Its prepared storage is released with the runtime, after the freeze.
        let pending = control.pending.take();
        let runtime = device.runtime.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: control serializes native mutation and freeze drained every IO reader.
        // Client metadata remains valid for late StopIO/RemoveDeviceClient calls.
        unsafe {
            device
                .capture_format
                .publish(device.capture_format.format(), Box::default());
            device.render_format.publish(device.render_format.format());
        }
        let running = device.running.load(Ordering::Acquire) != 0;
        drop(frozen);
        // SAFETY: dead devices cannot admit another native runtime access, so the runtime
        // and every mapping it owns are released after the freeze, on control.
        unsafe { (self.native.destroy)(runtime) };
        drop(pending);
        drop(control);
        drop(device);
        drop(guard);
        self.host.changed(id, ALIVE);
        if running {
            self.host.changed(id, RUNNING);
        }
        self.host.changed(1, LIST);
        self.reclaim();
        OK
    }

    pub(crate) fn client_pid(&self, id: u32, client: u32, pid: u32) {
        if let Some(device) = self.pin(id) {
            device
                .input_latency
                .identity(client, pid, self.inspection.process_start(pid));
        }
    }
    pub(crate) fn render_client(&self, id: u32, client: u32) {
        if let Some(device) = self.pin(id) {
            device.input_latency.render(client);
        }
    }
    pub(crate) fn notify_latency(&self, id: u32) {
        let changed = self.pin(id).is_some_and(|device| {
            device
                .input_latency
                .changed(device.rate.load(Ordering::Acquire))
        });
        if changed {
            self.host.changed(id, crate::abi::INPUT_LATENCY);
        }
    }
    pub(crate) fn reset_latency_authority(&self) {
        for id in self.devices() {
            if let Some(device) = self.pin(id) {
                let mut control = lock(&device.control);
                control.latency_lease = 0;
                control.latency_revision = 0;
                device.input_latency.publish(0, 0, None, None);
            }
            self.notify_latency(id);
        }
    }
    pub(crate) fn clear_latencies(&self) {
        for id in self.devices() {
            if let Some(device) = self.pin(id) {
                device.input_latency.publish(0, 0, None, None);
            }
            self.notify_latency(id);
        }
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn publish_latencies(
        &self,
        entries: &[mixrelay_macos_transport::control_inventory::InputLatency],
    ) {
        for id in self.devices() {
            let Some(device) = self.pin(id) else { continue };
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.endpoint_id == device.endpoint)
            {
                // Resolved before locking and while the engine is publishing, so its
                // incarnation is current.
                let start = self.inspection.process_start(entry.engine_pid);
                let mut control = lock(&device.control);
                if entry.lease < control.latency_lease
                    || (entry.lease == control.latency_lease
                        && entry.revision < control.latency_revision)
                {
                    continue;
                }
                control.latency_lease = entry.lease;
                control.latency_revision = entry.revision;
                device
                    .input_latency
                    .publish(entry.rate, entry.engine_pid, start, entry.frames);
            } else {
                device.input_latency.publish(0, 0, None, None);
            }
            drop(device);
            self.notify_latency(id);
        }
    }

    pub(crate) fn transport_policy(&self, bundles: Vec<String>) {
        if self.transport.policy(bundles) {
            for id in self.devices() {
                self.host.changed(id, crate::abi::TRANSPORT);
            }
        }
    }

    pub(crate) fn devices(&self) -> Vec<u32> {
        (0..MAX_DEVICES)
            .filter_map(|index| self.pin_slot(index))
            .filter(|device| device.alive.load(Ordering::Acquire))
            .map(|device| device.id)
            .collect()
    }

    pub(crate) fn lookup_uid(&self, uid: &str) -> Option<u32> {
        (0..MAX_DEVICES)
            .filter_map(|index| self.pin_slot(index))
            .find(|device| device.alive.load(Ordering::Acquire) && device.uid == uid)
            .map(|device| device.id)
    }

    pub(crate) fn snapshot(&self, object: u32) -> Option<Snapshot> {
        Some(self.pin(object)?.snapshot())
    }

    /// I/O-adjacent identity checks must not enter the metadata control mutex.
    /// The fixed-slot pin keeps the device alive until its atomic state is read.
    pub(crate) fn device_alive(&self, object: u32) -> bool {
        self.pin(object)
            .is_some_and(|device| device.id == object && device.alive.load(Ordering::Acquire))
    }

    pub(crate) fn replacement_endpoint(&self) -> Option<[u8; 16]> {
        for id in self.devices() {
            let Some(device) = self.pin(id) else { continue };
            let control = lock(&device.control);
            if !device.alive.load(Ordering::Acquire) || control.pending.is_some() {
                continue;
            }
            if unsafe { (self.native.requires_replacement)(device.runtime.load(Ordering::Acquire)) }
            {
                return Some(device.endpoint);
            }
        }
        None
    }

    pub(crate) fn attachment_failed(&self, endpoint: [u8; 16]) -> bool {
        self.endpoint(endpoint)
            .is_some_and(|device| lock(&device.control).attachment_failed)
    }

    pub(crate) fn attachment_applied(&self, endpoint: [u8; 16]) -> bool {
        self.endpoint(endpoint).is_some_and(|device| {
            let control = lock(&device.control);
            !control.attachment_failed && control.pending.is_none()
        })
    }

    pub(crate) fn set_stream_active(&self, stream: u32, active: bool) -> i32 {
        let Some(device) = self.pin(stream) else {
            return NOT_FOUND;
        };
        if !device.alive.load(Ordering::Acquire) {
            return NOT_FOUND;
        }
        // AcqRel publishes the new usage and observes the previous value in one
        // transition, so repeated host writes cannot create duplicate changes.
        let previous = if stream == device.id + 1 {
            device.input_active.swap(active, Ordering::AcqRel)
        } else if stream == device.id + 2 {
            device.output_active.swap(active, Ordering::AcqRel)
        } else {
            return INVALID;
        };
        drop(device);
        // CoreAudio reapplies stream usage while registering an IOProc. Echoing
        // an unchanged value back as a change reenters that reconciliation.
        if previous != active {
            self.host.changed(stream, STREAM_ACTIVE);
        }
        OK
    }

    pub(crate) fn output_control(&self, object: u32) -> Option<(f32, bool)> {
        let device = self.pin(object)?;
        (device.render_channels.load(Ordering::Acquire) != 0)
            .then(|| device.output_control.values())
    }

    pub(crate) fn set_output_volume(&self, object: u32, scalar: f32) -> i32 {
        let Some(device) = self.pin(object).filter(|device| {
            object == device.id + 3
                && device.alive.load(Ordering::Acquire)
                && device.render_channels.load(Ordering::Acquire) != 0
        }) else {
            return NOT_FOUND;
        };
        if crate::output_control::scalar_to_decibels(scalar).is_none() {
            return INVALID;
        }
        let changed = device.output_control.set_scalar(scalar);
        let id = device.id;
        drop(device);
        if changed {
            self.host.changed(id, OUTPUT_VOLUME);
        }
        OK
    }

    pub(crate) fn set_output_mute(&self, object: u32, muted: bool) -> i32 {
        let Some(device) = self.pin(object).filter(|device| {
            object == device.id + 4
                && device.alive.load(Ordering::Acquire)
                && device.render_channels.load(Ordering::Acquire) != 0
        }) else {
            return NOT_FOUND;
        };
        let changed = device.output_control.set_muted(muted);
        let id = device.id;
        drop(device);
        if changed {
            self.host.changed(id, OUTPUT_MUTE);
        }
        OK
    }

    fn reclaim(&self) {
        let _guard = lock(&self.generation);
        for slot in &self.slots {
            let pointer = slot.pointer.load(Ordering::SeqCst);
            if pointer.is_null() {
                continue;
            }
            let device = unsafe { &*pointer };
            if device.alive.load(Ordering::Acquire) || device.running.load(Ordering::Acquire) != 0 {
                continue;
            }
            if !lock(&device.control).clients.is_empty() {
                continue;
            }
            // First unpublish the pointer, then inspect readers. New readers can
            // only observe null. If old readers remain, defer rather than free.
            slot.pointer.store(std::ptr::null_mut(), Ordering::SeqCst);
            while slot.readers.load(Ordering::SeqCst) != 0 {
                std::thread::yield_now();
            }
            drop(unsafe { Box::from_raw(pointer) });
        }
    }

    pub(crate) fn timestamp(&self, id: u32, now: u64, output: &mut Timestamp) -> bool {
        let _audio = crate::transport::AudioScope::enter();
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return false;
        };
        let Some(_io) = device.io_pin() else {
            return false;
        };
        device.alive.load(Ordering::Acquire)
            && unsafe {
                (self.native.timestamp)(device.runtime.load(Ordering::Acquire), now, output)
            }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            let pointer = *slot.pointer.get_mut();
            if !pointer.is_null() {
                drop(unsafe { Box::from_raw(pointer) });
            }
        }
    }
}
