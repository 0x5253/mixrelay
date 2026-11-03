//! Graph control never mutates callback-owned conversion or its queue cursors.
//! The PCM callback acknowledges a generation only after resetting its state.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub(super) struct BridgeActivity {
    admitted: AtomicBool,
    requested: AtomicU64,
    applied: AtomicU64,
    transfer_epoch: AtomicU64,
}

impl Default for BridgeActivity {
    fn default() -> Self {
        Self {
            admitted: AtomicBool::new(false),
            requested: AtomicU64::new(1),
            applied: AtomicU64::new(1),
            transfer_epoch: AtomicU64::new(0),
        }
    }
}

impl BridgeActivity {
    /// The first graph use starts a fresh PCM epoch after native preparation.
    /// Subsequent plans sharing this device retain its admitted stream.
    pub(super) fn admit(&self) {
        // Only the serialized graph owner changes admission. No callback state
        // is published by this flag; the requested generation carries that fence.
        if !self.admitted.swap(true, Ordering::Relaxed) {
            let previous = self.requested.load(Ordering::Acquire);
            // Advance the generation without changing its enabled bit. The PCM
            // owner discards all preparation queues/history before acknowledging.
            self.requested
                .store(previous.wrapping_add(2), Ordering::Release);
        }
    }

    /// The serialized graph owner publishes one coherent block's direction state.
    pub(super) fn request(&self, enabled: bool) -> bool {
        let previous = self.requested.load(Ordering::Acquire);
        if (previous & 1 != 0) == enabled {
            return false;
        }
        self.requested
            .store(previous.wrapping_add(1), Ordering::Release);
        true
    }

    pub(super) fn ready(&self) -> bool {
        let requested = self.requested.load(Ordering::Acquire);
        requested & 1 != 0 && self.applied.load(Ordering::Acquire) == requested
    }

    /// OFF retains the native clock but has no PCM stream to synchronize.
    pub(super) fn enabled(&self) -> bool {
        self.requested.load(Ordering::Acquire) & 1 != 0
    }

    pub(super) fn read_epoch(&self, native_epoch: u64) -> u64 {
        // Both values use NEXT_DEVICE_EPOCH. The newest invalidation wins, but
        // an OFF queue reset never changes the continuing hardware pacing clock.
        native_epoch.max(self.transfer_epoch.load(Ordering::Acquire))
    }

    /// Only the PCM owner calls this; reset completes before PCM admission.
    pub(super) fn synchronize(&self, reset: impl FnOnce() -> u64) -> bool {
        let requested = self.requested.load(Ordering::Acquire);
        if self.applied.load(Ordering::Acquire) != requested {
            self.transfer_epoch.store(reset(), Ordering::Release);
            self.applied.store(requested, Ordering::Release);
        }
        requested & 1 != 0
    }
}

#[cfg(test)]
mod tests {
    use super::super::{bridge::DeviceBridge, ffi, tests::synthetic_device};
    use super::*;

    #[test]
    fn rapid_off_on_still_requires_worker_reset_and_unchanged_users_do_not() {
        let activity = BridgeActivity::default();
        assert!(activity.ready());
        assert!(!activity.request(true));
        assert!(activity.request(false));
        assert!(activity.request(true));
        assert!(!activity.ready());
        let mut resets = 0;
        assert!(activity.synchronize(|| {
            resets += 1;
            resets
        }));
        assert!(activity.ready());
        assert!(activity.synchronize(|| {
            resets += 1;
            resets
        }));
        assert_eq!(resets, 1);
        assert!(activity.request(false));
        assert!(!activity.synchronize(|| {
            resets += 1;
            resets
        }));
        assert!(!activity.ready());
        assert_eq!(resets, 2);
    }

    #[test]
    fn input_off_on_discards_queued_pcm_and_converter_history_before_fresh_read() {
        let (device, context) = synthetic_device(2, 48_000, 48_000, true);
        let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
        let pacing_epoch = context.epoch.load(Ordering::Acquire);
        let mut samples = [0.0; 64];
        device.read(&mut samples, 0);
        // SAFETY: this unpublished synthetic owner has no concurrent worker or
        // native callback; the test acts as its sole graph-ring producer.
        assert_eq!(
            unsafe {
                ffi::mr_device_io_bridge_write(
                    device.native.pointer.as_ptr(),
                    [0.25; 64].as_ptr(),
                    32,
                )
            },
            32
        );
        bridge.bridge.push_interleaved(&[0.25; 64]).unwrap();
        device.set_active(false);
        assert_native_fallback_blocked(&device);
        assert!(!bridge.synchronize_activity(&context));
        assert_eq!(bridge.bridge.buffered_input_frames(), 0);
        device.set_active(true);
        assert_native_fallback_blocked(&device);
        samples.fill(1.0);
        device.read(&mut samples, 0);
        assert_eq!(samples.map(f32::to_bits), [0.0_f32.to_bits(); 64]);
        assert!(bridge.synchronize_activity(&context));
        device.read(&mut samples, 0);
        assert_eq!(samples.map(f32::to_bits), [0.0_f32.to_bits(); 64]);
        // SAFETY: same exclusive synthetic producer after both reset acknowledgements.
        assert_eq!(
            unsafe {
                ffi::mr_device_io_bridge_write(
                    device.native.pointer.as_ptr(),
                    [0.75; 64].as_ptr(),
                    32,
                )
            },
            32
        );
        assert_eq!(device.read(&mut samples, 0).frames, 32);
        assert_eq!(samples.map(f32::to_bits), [0.75_f32.to_bits(); 64]);
        assert_eq!(context.epoch.load(Ordering::Acquire), pacing_epoch);
    }

    fn assert_native_fallback_blocked(device: &super::super::Device) {
        // SAFETY: the synthetic owner retains this slot; OFF dispatch returns
        // before reading native buffer pointers and has no published renderer.
        assert!(!unsafe {
            super::super::direct::dispatch(
                std::ptr::from_ref(device.native.render.as_ref())
                    .cast_mut()
                    .cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
                32,
                std::ptr::null(),
            )
        });
        // The native boundary, rather than the graph hook, gates unacknowledged PCM.
        assert!(!unsafe {
            super::super::worker::callback::dispatch(
                std::ptr::from_ref(device.native.pcm.as_ref())
                    .cast_mut()
                    .cast(),
                super::super::ffi::PcmPhase::Prepare as u32,
            )
        });
    }

    #[test]
    fn output_off_on_drains_only_its_worker_source_and_waits_for_acknowledgement() {
        let (device, context) = synthetic_device(2, 48_000, 48_000, false);
        let (unrelated, unrelated_context) = synthetic_device(2, 48_000, 48_000, false);
        let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
        assert_eq!(device.write(&[0.25; 64], 0).frames, 32);
        assert_eq!(unrelated.write(&[0.5; 64], 0).frames, 32);
        let other_epoch = unrelated_context.epoch.load(Ordering::Acquire);
        device.set_active(false);
        assert_native_fallback_blocked(&device);
        assert!(!bridge.synchronize_activity(&context));
        // SAFETY: immutable queue-count observations retain both native owners.
        assert_eq!(
            unsafe { ffi::mr_device_io_queued(device.native.pointer.as_ptr(), true) },
            0
        );
        assert_eq!(
            unsafe { ffi::mr_device_io_queued(unrelated.native.pointer.as_ptr(), true) },
            32
        );
        assert_eq!(unrelated_context.epoch.load(Ordering::Acquire), other_epoch);
        device.set_active(true);
        assert_native_fallback_blocked(&device);
        device.write(&[0.75; 64], 0);
        assert_eq!(
            unsafe { ffi::mr_device_io_queued(device.native.pointer.as_ptr(), true) },
            0
        );
        assert!(bridge.synchronize_activity(&context));
        device.write(&[0.75; 64], 0);
        assert_eq!(
            unsafe { ffi::mr_device_io_queued(device.native.pointer.as_ptr(), true) },
            32
        );
    }
}
