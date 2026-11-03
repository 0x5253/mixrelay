//! Shared application PCM ownership, committed credit, and native transport ABI.

use super::{CAPTURE_RING_CAPACITY_FRAMES, CaptureProbeStats};
use std::{ffi::CString, ptr::NonNull};

pub(super) use mixrelay_macos_transport::application_source_transport::SourceClockSnapshot as NativeClockSnapshot;
use mixrelay_macos_transport::application_source_transport::{
    SourceTransport as NativeApplicationSourceTransport, TransportError as NativeTransportError,
    mr_application_source_transport_clock, mr_application_source_transport_connect,
    mr_application_source_transport_destroy, mr_application_source_transport_publish_measurements,
    mr_application_source_transport_record_source_faults, mr_application_source_transport_reset,
    mr_application_source_transport_stop_native, mr_application_source_transport_write_clocked,
};

pub(super) struct ApplicationSourceTransport {
    native: NonNull<NativeApplicationSourceTransport>,
}

// SAFETY: one pump thread exclusively owns the mapping and uses lock-free shared atoms.
unsafe impl Send for ApplicationSourceTransport {}

impl ApplicationSourceTransport {
    pub(super) fn observe_capture(&mut self, health: &CaptureProbeStats) {
        if !health.running {
            self.stop_native();
        }
    }

    pub(super) fn publish_measurements(&mut self, producer: u32, converter: u32, drought: u32) {
        // SAFETY: this pump exclusively owns the live transport.
        unsafe {
            mr_application_source_transport_publish_measurements(
                self.native.as_ptr(),
                producer,
                converter,
                drought,
            );
        }
    }

    pub(super) fn stop_native(&mut self) {
        // SAFETY: sole pump owner; bounded atomic publication into validated trailer.
        unsafe {
            mr_application_source_transport_stop_native(self.native.as_ptr());
        };
    }
    pub(super) fn clock(&mut self) -> Option<NativeClockSnapshot> {
        let mut snapshot = NativeClockSnapshot::default();
        // SAFETY: sole worker owns the transport; Rust validates the versioned mapping.
        unsafe { mr_application_source_transport_clock(self.native.as_ptr(), &raw mut snapshot) }
            .then_some(snapshot)
    }

    pub(super) fn reset(&mut self, format: u64, native_rate: u32) -> bool {
        // SAFETY: only the producer worker can reset this transport's epoch.
        unsafe { mr_application_source_transport_reset(self.native.as_ptr(), format, native_rate) }
    }

    pub(super) fn record_source_faults(&mut self, epoch: u64, count: u64) -> bool {
        // SAFETY: this pump owns the transport; native code validates its lease
        // and exact epoch before publishing or retaining this fixed counter.
        unsafe {
            mr_application_source_transport_record_source_faults(self.native.as_ptr(), epoch, count)
        }
    }

    pub(super) fn connect(source_id: &str, channels: u16) -> Result<Self, String> {
        let source_id = CString::new(source_id)
            .map_err(|_| "application source ID contains a null byte".to_owned())?;
        let mut error = NativeTransportError::default();
        // SAFETY: the source ID remains valid for the synchronous XPC attachment request.
        let native = unsafe {
            mr_application_source_transport_connect(
                source_id.as_ptr(),
                u32::try_from(CAPTURE_RING_CAPACITY_FRAMES)
                    .map_err(|_| "application source capacity is invalid".to_owned())?,
                u32::from(channels),
                &raw mut error,
            )
        };
        let native = NonNull::new(native).ok_or_else(|| {
            format!(
                "application source transport failed: code={} system_error={}",
                error.code as i32, error.system_error
            )
        })?;
        Ok(Self { native })
    }

    pub(super) fn write(&mut self, samples: &[f32], times: &[u64], frames: usize) -> bool {
        let Ok(frames) = u32::try_from(frames) else {
            return false;
        };
        // SAFETY: this pump exclusively owns the transport and samples cover the frame count.
        unsafe {
            mr_application_source_transport_write_clocked(
                self.native.as_ptr(),
                samples.as_ptr(),
                times.as_ptr(),
                frames,
            )
        }
    }
}

impl Drop for ApplicationSourceTransport {
    fn drop(&mut self) {
        // SAFETY: the pump has unique ownership of this mapped transport.
        unsafe { mr_application_source_transport_destroy(self.native.as_ptr()) };
    }
}

#[cfg(test)]
#[path = "committed_credit_tests.rs"]
mod committed_credit_tests;

#[cfg(all(test, mixrelay_application_clock_fixture))]
#[path = "live_clock_tests.rs"]
mod live_clock_tests;
