//! Graph-owned loss notification, separate from PCM admission and native clocks.
use crate::macos_devices::ffi::GraphRead;
use mixrelay_audio_io::process::{InputActivity, TransferOutcome};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

const PRIMING: u8 = 0;
const ACTIVE: u8 = 1;
const LOST: u8 = 2;

/// Only the serialized graph consumer changes this fixed state. Atomic storage
/// keeps the shared Device handle Sync without granting another role access to
/// graph-owned mutable references. No PCM cursor or native lifecycle is changed.
#[derive(Default)]
pub(in crate::macos_devices) struct InputContinuity(AtomicU8);

impl InputContinuity {
    pub(super) fn lost(&self, source_sample_faults: u64) -> TransferOutcome {
        let previous = self.0.load(Relaxed);
        if previous != PRIMING {
            self.0.store(LOST, Relaxed);
        }
        TransferOutcome {
            frames: 0,
            discontinuity: previous == ACTIVE,
            activity: if previous == PRIMING {
                InputActivity::Priming
            } else {
                InputActivity::Inactive
            },
            source_sample_faults,
        }
    }

    pub(super) fn admit(&self, read: GraphRead) -> bool {
        let previous = self.0.load(Relaxed);
        // A rejected native request is the empty default; it cannot admit a
        // timeline or consume the pending recovery marker. Valid first/recovery
        // reads expose an epoch flag even when freshness discards every frame.
        if read.initial_epoch || read.discontinuity || read.frames != 0 {
            self.0.store(ACTIVE, Relaxed);
        }
        // The graph already reset this input's downstream histories at loss.
        // Native admission must still discard old PCM at the new epoch, but
        // its first recovery notification must not reset other mixed tails again.
        read.discontinuity && previous != LOST
    }
}

#[cfg(test)]
mod tests;
