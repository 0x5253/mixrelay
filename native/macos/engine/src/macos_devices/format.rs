//! Staged format transaction. Allocation precedes native mutation; commit publishes to workers.
use super::buffer_control::{self, HostBufferControl};
use super::buffer_transaction::{BufferFailure, BufferPlan};
use super::{MacDevices, backend_error, bridge::DeviceBridge, device::Device, spec::specs};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::HostAudioBackendError;
use mixrelay_engine_core::PreparedGraphFormat;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

/// Last committed client format, shared with resource creation/reconnection.
/// Writers are serialized by the format reservation, readers by the cache lock.
pub(super) struct PublishedFormat {
    rate: AtomicU32,
    buffer_frames: AtomicU32,
}
impl PublishedFormat {
    pub(super) fn new(rate: u32, buffer_frames: u32) -> Self {
        Self {
            rate: AtomicU32::new(rate),
            buffer_frames: AtomicU32::new(buffer_frames),
        }
    }
    pub(super) fn current(&self) -> mixrelay_audio_graph::RenderFormat {
        mixrelay_audio_graph::RenderFormat {
            sample_rate: self.rate.load(Ordering::Acquire),
            block_frames: self.buffer_frames.load(Ordering::Acquire),
        }
    }
}
impl MacDevices {
    /// Opens the replacement device set and its converters for `requested` while the
    /// current set keeps running. Nothing is published until the returned format commits.
    pub(super) fn prepare_device_format(
        &self,
        document: &GraphDocument,
        current: RenderFormat,
        requested: RenderFormat,
    ) -> Result<Box<dyn PreparedGraphFormat>, HostAudioBackendError> {
        self.require_preparation()?;
        let cache = self
            .cache
            .lock()
            .map_err(|_| backend_error("CoreAudio device cache poisoned"))?;
        self.handoff.reclaim();
        let reservation = self.handoff.reserve_format().ok_or_else(|| {
            backend_error("CoreAudio resource or format update is already pending")
        })?;
        let (devices, next_cache) = self.prepare_device_set(&cache, specs(document), requested)?;
        self.require_preparation()?;
        let format = PreparedDeviceFormat::prepare(
            devices.0.values().cloned(),
            self.format.clone(),
            current,
            requested,
            reservation,
        )?;
        Ok(Box::new(PreparedDeviceReplacement {
            format,
            devices,
            next_cache,
            cache: self.cache.clone(),
            handoff: self.handoff.clone(),
            hardware_clock: self.prepared_hardware_clock.clone(),
        }))
    }
}

pub(super) struct PreparedDeviceFormat {
    pub(super) reservation: super::handoff::FormatReservation,
    pub(super) devices: Vec<(Arc<Device>, DeviceBridge)>,
    pub(super) published: Arc<PublishedFormat>,
    pub(super) requested_rate: u32,
    pub(super) requested_buffer: u32,
    pub(super) previous_buffer: u32,
}

pub(super) struct PreparedDeviceReplacement {
    pub(super) format: PreparedDeviceFormat,
    pub(super) devices: super::DeviceSet,
    pub(super) next_cache: super::Cache,
    pub(super) cache: Arc<std::sync::Mutex<super::Cache>>,
    pub(super) handoff: Arc<super::ResourceHandoff<super::DeviceSet>>,
    pub(super) hardware_clock: Arc<std::sync::atomic::AtomicBool>,
}

impl PreparedGraphFormat for PreparedDeviceReplacement {
    fn configure_native(&mut self) -> Result<(), HostAudioBackendError> {
        self.format.configure_native()
    }

    fn commit(self: Box<Self>) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.hardware_clock
            .store(!self.next_cache.is_empty(), Ordering::Release);
        self.handoff
            .publish_reserved(&self.format.reservation, self.devices);
        Box::new(self.format).commit();
        *cache = self.next_cache;
    }
}
impl PreparedGraphFormat for PreparedDeviceFormat {
    fn configure_native(&mut self) -> Result<(), HostAudioBackendError> {
        if self.requested_buffer == self.previous_buffer {
            return Ok(());
        }
        // Every owner of one device set shares its backend's host.
        let Some(host) = self
            .devices
            .first()
            .map(|(device, _)| device.native.host.clone())
        else {
            return Ok(());
        };
        let mut changes = Vec::new();
        for (device, _) in &self.devices {
            changes.push(
                buffer_control::prepare(&device.native, self.requested_buffer)
                    .map_err(backend_error)?,
            );
        }
        BufferPlan::prepare(changes, self.requested_buffer)
            .and_then(|plan| plan.apply(&mut HostBufferControl(&*host)))
            .map_err(|failure| backend_error(match failure {
                BufferFailure::InconsistentSnapshot => "CoreAudio native buffer changed during preparation",
                BufferFailure::Restored => "CoreAudio buffer change failed native readback; previous buffer restored",
                BufferFailure::RollbackFailed => "CoreAudio buffer change failed; previous native buffer could not be restored",
            }))
    }
    fn commit(self: Box<Self>) {
        for (device, bridge) in self.devices {
            device.native.request_buffer(self.requested_buffer);
            // The controller serializes format transactions; only the worker
            // takes this slot. All converter allocation and validation happened
            // before native negotiation, and cancellation simply drops token.
            let mut pending = device
                .pending_bridge
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *pending = Some(bridge);
            device
                .graph_rate
                .store(self.requested_rate, Ordering::Release);
            drop(pending);
            if let Some(worker) = &device.worker {
                worker.thread().unpark();
            }
        }
        self.published
            .rate
            .store(self.requested_rate, Ordering::Release);
        self.published
            .buffer_frames
            .store(self.requested_buffer, Ordering::Release);
    }
}

impl PreparedDeviceFormat {
    pub(super) fn prepare(
        owners: impl Iterator<Item = Arc<Device>>,
        published: Arc<PublishedFormat>,
        current: mixrelay_audio_graph::RenderFormat,
        requested: mixrelay_audio_graph::RenderFormat,
        reservation: super::handoff::FormatReservation,
    ) -> Result<Self, HostAudioBackendError> {
        let mut devices = Vec::new();
        for device in owners {
            if device
                .pending_bridge
                .lock()
                .map_err(|_| backend_error("CoreAudio bridge mailbox poisoned"))?
                .is_some()
            {
                return Err(backend_error("CoreAudio bridge format publication pending"));
            }
            let bridge = DeviceBridge::prepare(
                device.channels,
                device.input,
                device.native_rate.load(Ordering::Acquire),
                requested.sample_rate,
                device.clock_aligned.load(Ordering::Acquire),
            )
            .map_err(backend_error)?;
            devices.push((device, bridge));
        }
        Ok(Self {
            reservation,
            devices,
            published,
            requested_rate: requested.sample_rate,
            requested_buffer: requested.block_frames,
            previous_buffer: current.block_frames,
        })
    }
}

#[cfg(test)]
mod tests;
