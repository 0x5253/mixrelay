//! Host-approved format and mapping transactions with bounded retry state.
use super::{CONFIG_ACK_TIMEOUT_TICKS, Registry};

/// Settlements kept for the control tick. The tick runs every 100 ms and every request
/// needs a coreaudiod round trip, so the oldest is never dropped before it is sent.
pub(super) const SETTLED_RETAINED: usize = 256;
use crate::{
    abi::{CAPACITY, CONFIG_ACTION, FORMAT, INVALID, NAME, NATIVE_ERROR, NOT_FOUND, OK},
    device::{ConversionScratch, Device, Pending, Settled, lock},
};
use mixrelay_pcm_ring::SampleFormat;
use std::sync::atomic::Ordering;

impl Registry {
    pub(crate) fn set_rate(&self, id: u32, rate: u32) -> i32 {
        self.queue_configuration(id, rate, None)
    }

    pub(crate) fn set_format(&self, stream: u32, rate: u32, format: SampleFormat) -> i32 {
        let Some(device) = self.pin(stream) else {
            return NOT_FOUND;
        };
        let index = if stream == device.id + 1
            && device.capture_channels.load(Ordering::Acquire) != 0
        {
            0
        } else if stream == device.id + 2 && device.render_channels.load(Ordering::Acquire) != 0 {
            1
        } else {
            return INVALID;
        };
        self.queue_configuration(device.id, rate, Some((index, format)))
    }

    fn queue_configuration(
        &self,
        id: u32,
        rate: u32,
        requested_format: Option<(usize, SampleFormat)>,
    ) -> i32 {
        if !crate::formats::SAMPLE_RATES.contains(&rate) {
            return INVALID;
        }
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        if !device.alive.load(Ordering::Acquire) {
            return NOT_FOUND;
        }
        let current_formats = [
            device.capture_format.format(),
            device.render_format.format(),
        ];
        if control.pending.is_none()
            && device.rate.load(Ordering::Acquire) == rate
            && requested_format.is_none_or(|(index, format)| current_formats[index] == format)
        {
            return OK;
        }
        let generation = control.next_generation;
        let Some(next) = generation.checked_add(1) else {
            return CAPACITY;
        };
        control.next_generation = next;
        let (attachment, name, mut formats) = control.pending.take().map_or_else(
            || (None, control.name.clone(), current_formats),
            |pending| (pending.attachment, pending.name, pending.formats),
        );
        if let Some((index, format)) = requested_format {
            formats[index] = format;
        }
        control.pending = Some(Pending {
            generation,
            requested: false,
            retry_after: 0,
            failures: 0,
            attachment,
            name,
            rate,
            formats,
        });
        drop(control);
        drop(device);
        self.request(id);
        OK
    }

    pub(super) fn request(&self, id: u32) {
        let Some(device) = self.pin(id) else { return };
        let mut control = lock(&device.control);
        let Some(pending) = control.pending.as_mut() else {
            return;
        };
        if pending.requested || pending.retry_after > 0 {
            return;
        }
        pending.requested = true;
        pending.retry_after = CONFIG_ACK_TIMEOUT_TICKS;
        let generation = pending.generation;
        drop(control);
        drop(device);
        if self.host.request(id, generation) {
            return;
        }
        let Some(device) = self.pin(id) else { return };
        let mut control = lock(&device.control);
        if let Some(pending) = control
            .pending
            .as_mut()
            .filter(|pending| pending.generation == generation)
        {
            pending.requested = false;
            pending.failures = (pending.failures + 1).min(6);
            pending.retry_after = 1 << pending.failures;
        }
    }

    pub(crate) fn tick(&self) {
        for id in self.devices() {
            if let Some(device) = self.pin(id)
                && let Some(pending) = lock(&device.control).pending.as_mut()
            {
                pending.retry_after = pending.retry_after.saturating_sub(1);
                if pending.retry_after == 0 {
                    pending.requested = false;
                }
            }
            self.request(id);
            self.notify_latency(id);
            if let Some(device) = self.pin(id) {
                let mut control = lock(&device.control);
                if control.unprovisioned != 0 {
                    self.provision_running(&device, &mut control);
                }
            }
        }
        self.reclaim();
    }

    /// Records how one rate, format or mapping request ended, so the Driver service
    /// rolls back a rejected request instead of waiting out its timeout.
    pub(crate) fn settle(&self, device: &Device, pending: &Pending, applied: bool) {
        // The sequence is taken under the queue mutex, so concurrent settlements on two
        // devices enter the queue in sequence order. Relaxed: the mutex orders them.
        let mut settled = lock(&self.settled);
        let sequence = self.settlements.fetch_add(1, Ordering::Relaxed) + 1;
        if settled.len() == SETTLED_RETAINED {
            settled.pop_front();
        }
        settled.push_back(Settled {
            endpoint: device.endpoint,
            sequence,
            rate: pending.rate,
            formats: pending.formats.map(|format| format as u32),
            applied,
        });
        drop(settled);
        if let Some(signal) = self.control_signal.get() {
            signal.notify();
        }
    }

    /// Settled requests newer than `after`, oldest first, read on the control queue.
    pub(crate) fn settled_after(&self, after: u64) -> Vec<Settled> {
        lock(&self.settled)
            .iter()
            .filter(|settled| settled.sequence > after)
            .copied()
            .collect()
    }

    /// Commits a configuration change, then gives each client holding IO a queue of the
    /// committed shape before control returns to coreaudiod. Core Audio only pauses IO
    /// around this call; it does not call StopIO/StartIO, so a client whose queue the
    /// commit detached would otherwise render into nothing.
    pub(crate) fn perform(&self, id: u32, action: u64, generation: u64) -> i32 {
        let status = self.commit(id, action, generation, Device::prepare_scratch);
        if status == OK {
            self.reprovision(id);
        }
        status
    }

    /// The frozen, allocation-free step of `perform`.
    pub(crate) fn commit(
        &self,
        id: u32,
        action: u64,
        generation: u64,
        prepare: impl FnOnce(
            &Device,
            &Pending,
        ) -> Result<ConversionScratch, std::collections::TryReserveError>,
    ) -> i32 {
        if action != CONFIG_ACTION {
            return INVALID;
        }
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        if !device.alive.load(Ordering::Acquire) {
            return NOT_FOUND;
        }
        if control
            .pending
            .as_ref()
            .is_none_or(|pending| pending.generation != generation)
        {
            return OK;
        }
        let Some(mut pending) = control.pending.take() else {
            return OK;
        };
        // Allocate bounded directional storage on control, before draining callbacks.
        let Ok(scratch) = prepare(&device, &pending) else {
            control.attachment_failed |= pending.attachment.is_some();
            self.settle(&device, &pending, false);
            return CAPACITY;
        };
        let frozen = device.freeze();
        let runtime = device.runtime.load(Ordering::Acquire);
        let success = if let Some(attachment) = &mut pending.attachment {
            attachment.prepared.take().is_some_and(|prepared| {
                // SAFETY: control and freeze retain the live runtime; replace consumes
                // the mapping prepared for exactly this attachment and maps nothing.
                unsafe { (self.native.replace)(runtime, prepared.into_raw(), pending.rate) }
            })
        } else {
            device.rate.load(Ordering::Acquire) == pending.rate
                // SAFETY: control and freeze exclude callbacks during this rate transition.
                || unsafe { (self.native.set_rate)(runtime, pending.rate) }
        };
        if success {
            // SAFETY: control and freeze are held, all IO pins are drained, and the native
            // rate/mapping transition succeeded. Abort/failure never publishes these buffers.
            unsafe {
                device.publish_configuration(&pending, scratch);
            }
        }
        drop(frozen);
        // SAFETY: control retains the live runtime; the freeze has ended.
        unsafe { (self.native.release)(runtime) };
        if !success {
            control.attachment_failed |= pending.attachment.is_some();
            self.settle(&device, &pending, false);
            return NATIVE_ERROR;
        }
        self.settle(&device, &pending, true);
        let renamed = control.name != pending.name;
        control.name = pending.name;
        drop(control);
        drop(device);
        self.host.changed(id, FORMAT);
        self.notify_latency(id);
        if renamed {
            self.host.changed(id, NAME);
        }
        OK
    }

    pub(crate) fn abort(&self, id: u32, action: u64, generation: u64) -> i32 {
        if action != CONFIG_ACTION {
            return INVALID;
        }
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return NOT_FOUND;
        };
        let mut control = lock(&device.control);
        if let Some(pending) = control
            .pending
            .take_if(|pending| pending.generation == generation)
        {
            control.attachment_failed |= pending.attachment.is_some();
            self.settle(&device, &pending, false);
        }
        OK
    }
}
