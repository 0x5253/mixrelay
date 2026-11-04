//! Control-to-audio handoff of prepared native owners.
//!
//! Control publishes one prepared update; the audio thread validates and swaps it into
//! the active owners, then parks the displaced owners for off-RT disposal. No Box is
//! allocated or freed on the audio thread because destruction may wait for `pw-cli`.
use crate::{
    applications::CaptureSet, devices::DeviceStreams, endpoints, endpoints::EndpointSet,
    endpoints::EndpointSpecs, processing::failure::Failure,
};
use mixrelay_audio_graph::{ApplicationCaptureRequest, GraphDocument};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::resource_requirements::{DeviceRequirements, ResourceRequirements};
use mixrelay_linux_pipewire::GraphInputs;
use std::{
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};

/// Native resources a graph document requires; compared to skip unchanged edits.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct ResourceSpec {
    pub(crate) endpoints: EndpointSpecs,
    pub(crate) captures: Vec<(String, ApplicationCaptureRequest)>,
    pub(crate) devices: DeviceRequirements,
}

impl ResourceSpec {
    pub(crate) fn from_document(document: &GraphDocument) -> Result<Self, HostAudioBackendError> {
        let requirements = ResourceRequirements::from_document(document);
        Ok(Self {
            endpoints: endpoints::specs(requirements.endpoints)?,
            captures: requirements.captures,
            devices: requirements.devices,
        })
    }
}

/// Owners prepared for one plan revision. `None` keeps the active owners of that kind.
pub(crate) struct ResourceUpdate {
    pub(crate) revision: u64,
    pub(crate) endpoints: Option<Box<EndpointSet>>,
    pub(crate) captures: Option<Box<CaptureSet>>,
    pub(crate) devices: Option<Box<DeviceStreams>>,
}

impl ResourceUpdate {
    pub(crate) fn bind_inputs(&mut self, inputs: &GraphInputs) -> Result<(), String> {
        inputs.ensure_live().map_err(|error| error.to_string())?;
        if let Some(endpoints) = &mut self.endpoints {
            endpoints.bind_inputs(inputs)?;
        }
        if let Some(captures) = &mut self.captures {
            captures.bind_inputs(inputs)?;
        }
        if let Some(devices) = &mut self.devices {
            devices.bind_inputs(inputs)?;
        }
        Ok(())
    }

    /// Validates the whole transaction, then swaps every prepared owner into the
    /// active slots. Afterwards `self` holds the displaced owners.
    pub(super) fn transfer_into(
        &mut self,
        revision: u64,
        endpoints: &mut Box<EndpointSet>,
        captures: &mut Box<CaptureSet>,
        devices: &mut Box<DeviceStreams>,
    ) -> Result<(), Failure> {
        if self.revision != revision {
            return Err(Failure::ResourceRevision);
        }
        if self
            .captures
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(captures))
        {
            return Err(Failure::RetainedCaptures);
        }
        if self
            .devices
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(devices))
        {
            return Err(Failure::RetainedDevices);
        }
        if self
            .endpoints
            .as_ref()
            .is_some_and(|next| !next.can_retain_from(endpoints))
        {
            return Err(Failure::RetainedEndpoints);
        }
        if let Some(next) = &mut self.endpoints {
            next.retain_from(endpoints);
            std::mem::swap(endpoints, next);
        }
        if let Some(next) = &mut self.captures {
            next.retain_from(captures);
            std::mem::swap(captures, next);
        }
        if let Some(next) = &mut self.devices {
            next.retain_from(devices);
            std::mem::swap(devices, next);
        }
        Ok(())
    }
}

/// One pending slot written by control and one retired slot written by the audio thread.
pub(super) struct UpdateMailbox<T> {
    pending: AtomicPtr<T>,
    retired: AtomicPtr<T>,
}

impl<T> Default for UpdateMailbox<T> {
    fn default() -> Self {
        Self {
            pending: AtomicPtr::new(ptr::null_mut()),
            retired: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

impl<T> UpdateMailbox<T> {
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.load(Ordering::Acquire).is_null()
    }

    /// Publishes `update` unless another one is still pending; a rejected update is
    /// returned to the caller and dropped on its control thread.
    pub(super) fn publish(&self, update: T) -> Result<(), Box<T>> {
        let pointer = Box::into_raw(Box::new(update));
        self.pending
            .compare_exchange(
                ptr::null_mut(),
                pointer,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            // SAFETY: publication failed, so this thread still owns the allocation.
            .map_err(|_| unsafe { Box::from_raw(pointer) })
    }

    /// Stores an update whose publication the caller reserved exclusively, after
    /// checking that nothing was pending.
    pub(super) fn publish_reserved(&self, update: Box<T>) {
        self.pending.store(Box::into_raw(update), Ordering::Release);
    }

    /// Runs `edit` on the pending update, for binding inputs before the audio thread
    /// starts. Returns `None` when nothing is pending.
    ///
    /// # Safety
    /// No audio consumer may run and no other control thread may publish during `edit`.
    pub(super) unsafe fn edit_pending<R>(&self, edit: impl FnOnce(&mut T) -> R) -> Option<R> {
        // SAFETY: the caller guarantees exclusive access to the unique pending envelope.
        unsafe { self.pending.load(Ordering::Acquire).as_mut() }.map(edit)
    }

    /// Audio thread: applies the pending update through `transfer`. A failed transfer
    /// leaves it pending; a successful one parks it as retired until control reclaims it.
    /// A retired update not yet reclaimed is bounded backpressure, not a failed cycle.
    pub(super) fn activate(
        &self,
        transfer: impl FnOnce(&mut T) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        if !self.retired.load(Ordering::Acquire).is_null() {
            return Err(Failure::RetirementPending);
        }
        let pending = self.pending.swap(ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            return Ok(());
        }
        // SAFETY: `publish` created this pointer and this swap removed the only stored
        // owner. The envelope is reused as the retirement record, so nothing is freed here.
        let mut update = unsafe { Box::from_raw(pending) };
        let result = transfer(&mut *update);
        let slot = if result.is_ok() {
            &self.retired
        } else {
            &self.pending
        };
        slot.store(Box::into_raw(update), Ordering::Release);
        result
    }

    /// Control: drops the update the audio thread retired, if any.
    pub(super) fn reclaim_retired(&self) {
        let retired = self.retired.swap(ptr::null_mut(), Ordering::AcqRel);
        if !retired.is_null() {
            // SAFETY: the swap removed the only pointer the audio thread published.
            drop(unsafe { Box::from_raw(retired) });
        }
    }
}

impl<T> Drop for UpdateMailbox<T> {
    fn drop(&mut self) {
        for slot in [self.pending.get_mut(), self.retired.get_mut()] {
            if !slot.is_null() {
                // SAFETY: dropping the mailbox is exclusive; no other owner can reach it.
                drop(unsafe { Box::from_raw(*slot) });
            }
        }
    }
}

#[cfg(test)]
mod tests;
