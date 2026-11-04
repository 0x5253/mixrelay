//! The graph-facing view of one published device generation: binding lookup, PCM transfer
//! and capture alignment. Offline selections keep their graph slot with silent transfers.
mod clock;
#[cfg(test)]
mod tests;

use super::{device::Device, ffi, recovery, spec::Spec};
use mixrelay_audio_io::process::TransferOutcome;
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};

/// Online native owners, and offline selections that still hold their graph slot.
#[derive(Default)]
pub(crate) struct DeviceSet(
    pub(super) BTreeMap<String, Arc<Device>>,
    pub(super) BTreeMap<String, Spec>,
);
impl DeviceSet {
    pub(crate) fn reserve_capture_alignment(&self, rate: u32, frames: u32, enabled: bool) {
        // The sinc bridge publishes 128-frame slabs. Retain one further slab
        // beyond producer/consumer bursts; until observed, budget two slabs.
        const CONVERTER_SLAB: u32 = 128;
        let burst = self
            .0
            .values()
            .filter(|device| device.input && device.ready() && device.activity.enabled())
            .fold(frames.max(CONVERTER_SLAB * 2), |largest, device| {
                // SAFETY: this set retains the native owner; geometry is atomic.
                let native =
                    unsafe { ffi::mr_device_io_quantum(device.native.pointer.as_ptr(), false) };
                let native_rate = device.native_rate.load(Ordering::Acquire).max(1);
                let mapped = (u64::from(native) * u64::from(rate)).div_ceil(u64::from(native_rate));
                // SAFETY: this is the serialized graph consumer; successive native
                // IOProcs may have published several slabs before this graph cycle.
                let published = unsafe {
                    mixrelay_macos_transport::device_io::capture_burst(
                        device.native.pointer.as_ptr().cast(),
                        device
                            .activity
                            .read_epoch(device.epoch.load(Ordering::Acquire)),
                    )
                };
                largest
                    .max(u32::try_from(mapped).unwrap_or(u32::MAX))
                    .max(published)
            });
        for device in self.0.values().filter(|device| device.input) {
            // SAFETY: atomic reserve update; callback retains all mutable conversion state.
            unsafe {
                mixrelay_macos_transport::device_io::set_capture_window(
                    device.native.pointer.as_ptr().cast(),
                    if enabled {
                        burst
                            .saturating_add(frames)
                            .saturating_add(CONVERTER_SLAB)
                            .saturating_mul(2)
                    } else {
                        0
                    },
                );
            }
        }
    }
    pub(crate) fn binding_present(&self, id: &str, input: bool) -> bool {
        self.binding_key(id, input)
            .and_then(|key| self.0.get(key))
            .is_some_and(|device| device.ready())
    }
    pub(crate) fn capture_time(&self, id: &str, frames: u32) -> Option<u64> {
        let id = self.binding_key(id, true)?;
        let device = self.0.get(id)?;
        if !device.ready() || !device.activity.ready() {
            return None;
        }
        // SAFETY: this set retains the input owner; the serialized graph consumer
        // acknowledges its PCM epoch before observing the matching coordinates.
        unsafe {
            mixrelay_macos_transport::device_io::input_time(
                device.native.pointer.as_ptr().cast(),
                frames,
            )
        }
    }

    pub(crate) fn read_aligned(
        &self,
        id: &str,
        samples: &mut [f32],
        timing: (u64, u64, u32),
    ) -> Option<TransferOutcome> {
        let id = self.binding_key(id, true)?;
        if let Some(spec) = self.1.get(id) {
            samples.fill(0.0);
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        self.0
            .get(id)
            .map(|device| device.read_timed(samples, timing.0, Some((timing.1, timing.2))))
    }
    /// Publish the pacing callback's complete burst, not each split DSP block.
    pub(crate) fn set_graph_quantum(&self, frames: u32) {
        for device in self.0.values() {
            device.graph_quantum.store(frames, Ordering::Release);
        }
    }
    pub(super) fn binding_key(&self, id: &str, input: bool) -> Option<&str> {
        if let Some((key, _)) = self.0.get_key_value(id) {
            return Some(key);
        }
        if let Some((key, _)) = self.1.get_key_value(id) {
            return Some(key);
        }
        let suffix = if input { "/capture" } else { "/render" };
        let bounds = (std::ops::Bound::Included(id), std::ops::Bound::Unbounded);
        self.0
            .range::<str, _>(bounds)
            .map(|(key, _)| key)
            .take_while(|key| key.starts_with(id))
            .chain(
                self.1
                    .range::<str, _>(bounds)
                    .map(|(key, _)| key)
                    .take_while(|key| key.starts_with(id)),
            )
            .find(|key| key.strip_suffix(suffix) == Some(id))
            .map(String::as_str)
    }

    /// Applies the compiled binding gate to only its capture or render client.
    pub(crate) fn set_binding_active(&self, id: &str, input: bool, enabled: bool) {
        if let Some(key) = self.binding_key(id, input)
            && let Some(device) = self.0.get(key)
        {
            device.activity.admit();
            device.set_active(enabled);
        }
    }

    pub(crate) fn is_offline(&self, id: &str) -> bool {
        self.1.contains_key(id)
    }

    /// Observes disconnected/disabled clients without changing device identity.
    pub(crate) fn is_binding_inactive(&self, id: &str, input: bool) -> bool {
        self.binding_key(id, input).is_some_and(|key| {
            self.1.contains_key(key)
                || self
                    .0
                    .get(key)
                    .is_some_and(|device| !device.ready() || !device.activity.enabled())
        })
    }
    pub(crate) fn read(&self, id: &str, samples: &mut [f32], host: u64) -> Option<TransferOutcome> {
        let id = self.binding_key(id, true)?;
        if let Some(spec) = self.1.get(id) {
            samples.fill(0.0);
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        self.0.get(id).map(|device| device.read(samples, host))
    }
    pub(crate) fn write(&self, id: &str, samples: &[f32], host: u64) -> Option<TransferOutcome> {
        let id = self.binding_key(id, false)?;
        if let Some(spec) = self.1.get(id) {
            return Some(recovery::offline_transfer(
                samples.len(),
                usize::from(spec.channels),
            ));
        }
        if let Some(device) = self.0.get(id)
            && !device.ready()
        {
            return Some(recovery::offline_transfer(samples.len(), device.channels));
        }
        self.0.get(id).map(|device| device.write(samples, host))
    }
}
