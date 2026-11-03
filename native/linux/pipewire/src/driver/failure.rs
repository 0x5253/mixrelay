//! First native callback fault, published once before control-side diagnostics.
use super::DriverProcessFailure;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

#[derive(Default)]
pub(super) struct FailurePublication {
    code: AtomicI32,
    native_error: AtomicI32,
    native_frames: AtomicU32,
    native_rate: AtomicU32,
    stream_rate: AtomicU32,
    capacity_frames: AtomicU32,
}

impl FailurePublication {
    pub(super) fn is_set(&self) -> bool {
        // Only the serialized callback writes; this is its own terminal gate.
        self.code.load(Ordering::Relaxed) != 0
    }

    pub(super) fn publish(&self, failure: DriverProcessFailure) {
        if self.is_set() {
            return;
        }
        let code = match failure {
            DriverProcessFailure::MissingPosition => 1,
            DriverProcessFailure::InvalidPosition => 2,
            DriverProcessFailure::DependencyQueue(error) => {
                self.native_error.store(error, Ordering::Relaxed);
                4
            }
            DriverProcessFailure::FrameBudget {
                native_frames,
                native_rate,
                stream_rate,
                capacity_frames,
            } => {
                // The first publication's release tag freezes this atomic payload.
                self.native_frames.store(native_frames, Ordering::Relaxed);
                self.native_rate.store(native_rate, Ordering::Relaxed);
                self.stream_rate.store(stream_rate, Ordering::Relaxed);
                self.capacity_frames
                    .store(capacity_frames, Ordering::Relaxed);
                3
            }
        };
        // Control's acquire tag observes every payload field; no writer changes it again.
        self.code.store(code, Ordering::Release);
    }

    pub(super) fn read(&self) -> Option<DriverProcessFailure> {
        // A nonzero release tag publishes a complete immutable first fault.
        match self.code.load(Ordering::Acquire) {
            0 => None,
            1 => Some(DriverProcessFailure::MissingPosition),
            2 => Some(DriverProcessFailure::InvalidPosition),
            3 => Some(DriverProcessFailure::FrameBudget {
                native_frames: self.native_frames.load(Ordering::Relaxed),
                native_rate: self.native_rate.load(Ordering::Relaxed),
                stream_rate: self.stream_rate.load(Ordering::Relaxed),
                capacity_frames: self.capacity_frames.load(Ordering::Relaxed),
            }),
            4 => Some(DriverProcessFailure::DependencyQueue(
                self.native_error.load(Ordering::Relaxed),
            )),
            code => Some(DriverProcessFailure::DependencyQueue(code)),
        }
    }
}
