//! Pacing-clock selection: which native owner paces the graph and which peers share its clock.
use super::{super::device::Device, DeviceSet};
use crate::macos_devices::ffi::{self, mr_device_io_native_frames};
use std::sync::{Arc, atomic::Ordering};

pub(crate) struct HardwareClockProgress {
    pub(crate) owner_id: u64,
    pub(crate) epoch: u64,
    pub(crate) native_frames: u64,
    pub(crate) native_rate: u32,
    pub(crate) quantum: u32,
}

impl DeviceSet {
    pub(crate) fn frame_period(&self, rate: u32) -> Option<f64> {
        let device = self.clock_master()?;
        // SAFETY: the set retains the native pacing owner during this atomic observation.
        let native = unsafe {
            mixrelay_macos_transport::device_io::frame_nanoseconds(
                device.native.pointer.as_ptr().cast(),
            )
        };
        (native.is_finite() && native > 0.0).then(|| {
            native
                * f64::from(
                    device
                        .native_rate
                        .load(std::sync::atomic::Ordering::Acquire),
                )
                / f64::from(rate)
        })
    }
    /// Explicit graph pacing identity, never inferred from nominal sample rate.
    pub(crate) fn set_clock_master(&self, owner_id: Option<u64>) {
        let master = self.0.values().find(|device| {
            owner_id == Some(device.identity) && !device.failed.load(Ordering::Acquire)
        });
        let master_clock = master.map_or(0, |device| device.clock_identity.load(Ordering::Acquire));
        let master_rate = master.map_or(0, |device| device.native_rate.load(Ordering::Acquire));
        for device in self.0.values() {
            // Two IOProcs of the same non-aggregate device share its timeline.
            // A shared nominal rate or unknown clock domain is never proof.
            // User aggregates stay independent until their subdevice policy is
            // verified; merely sharing an Aggregate object ID is insufficient.
            let aligned = master.is_some()
                && !device.failed.load(Ordering::Acquire)
                && (owner_id == Some(device.identity)
                    || (master_clock != 0
                        && device.clock_identity.load(Ordering::Acquire) == master_clock
                        && master_rate != 0
                        && device.native_rate.load(Ordering::Acquire) == master_rate));
            device.clock_aligned.store(aligned, Ordering::Release);
        }
    }
    /// Reports readiness of one direction's converter, or no native owner.
    pub(crate) fn clock_synchronized(&self, id: &str, input: bool) -> Option<bool> {
        self.binding_key(id, input)
            .and_then(|key| self.0.get(key))
            .map(|device| device.ready() && device.bridge_ready.load(Ordering::Acquire))
    }
    /// A pacing candidate is not proof that unrelated clocks are synchronized.
    /// Prefer output demand; capture-only graphs can use a healthy input clock.
    pub(crate) fn hardware_clock_progress(&self) -> Option<HardwareClockProgress> {
        self.clock_master().map(|device| HardwareClockProgress {
            owner_id: device.identity,
            epoch: device.epoch.load(Ordering::Acquire),
            native_frames: unsafe { mr_device_io_native_frames(device.native.pointer.as_ptr()) },
            native_rate: device.native_rate.load(Ordering::Acquire),
            // SAFETY: the device set retains Native; quantum is an atomic snapshot.
            quantum: unsafe { ffi::mr_device_io_quantum(device.native.pointer.as_ptr(), false) },
        })
    }

    fn clock_master(&self) -> Option<&Device> {
        self.0
            .values()
            .map(Arc::as_ref)
            .filter(|device| {
                !device.failed.load(Ordering::Acquire)
                    && device.native_rate.load(Ordering::Acquire) != 0
            })
            .min_by_key(|device| device.input)
    }

    /// Do not render a device-paced plan using a saved or previous clock rate.
    /// Independent secondary devices may still need conversion; only the actual
    /// pacing owner's rate determines the plan. Offline branches do not gate it.
    pub(crate) fn processing_ready(&self, sample_rate: u32) -> bool {
        self.clock_master().is_none_or(|device| {
            device.ready()
                && device.graph_rate.load(Ordering::Acquire) == sample_rate
                && device.native_rate.load(Ordering::Acquire) == sample_rate
        })
    }
    /// The pacing device's latest callback label. A graph cycle labels all its reads and
    /// writes with this one time, so the label cancels between a route's source and sink
    /// FIFOs and their passages sum to the frame's actual bridge-to-bridge delay.
    pub(crate) fn graph_host(&self) -> u64 {
        self.clock_master()
            .map(Device::callback_host)
            .filter(|host| *host != 0)
            .unwrap_or_else(mixrelay_realtime::host_time)
    }
}
