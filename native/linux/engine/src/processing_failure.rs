//! Fixed-size processing failure causes, published on RT and read on control.

use mixrelay_audio_engine::ProcessBlockError;
use mixrelay_audio_io::process::CycleError;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum Failure {
    InvalidFormat = 1,
    SampleRateMismatch,
    CapacityExceeded,
    ClockDiscontinuity,
    TimestampRegressed,
    InvalidTransfer,
    FrameCountExceeded,
    SourceCountMismatch,
    SinkCountMismatch,
    SourceBufferTooShort,
    SinkBufferTooShort,
    EndpointSilenceCapacity,
    ResourceRevision,
    RetainedCaptures,
    RetainedDevices,
    RetainedEndpoints,
    RetirementPending,
    OutputReleaseRejected,
}

impl From<CycleError> for Failure {
    fn from(error: CycleError) -> Self {
        match error {
            CycleError::InvalidFormat => Self::InvalidFormat,
            CycleError::SampleRateMismatch => Self::SampleRateMismatch,
            CycleError::CapacityExceeded => Self::CapacityExceeded,
            CycleError::ClockDiscontinuity => Self::ClockDiscontinuity,
            CycleError::TimestampRegressed => Self::TimestampRegressed,
        }
    }
}

impl From<ProcessBlockError> for Failure {
    fn from(error: ProcessBlockError) -> Self {
        match error {
            ProcessBlockError::InvalidCycle(error) => error.into(),
            ProcessBlockError::InvalidTransfer => Self::InvalidTransfer,
            ProcessBlockError::FrameCountExceeded { .. } => Self::FrameCountExceeded,
            ProcessBlockError::SourceCountMismatch { .. } => Self::SourceCountMismatch,
            ProcessBlockError::SinkCountMismatch { .. } => Self::SinkCountMismatch,
            ProcessBlockError::SourceBufferTooShort { .. } => Self::SourceBufferTooShort,
            ProcessBlockError::SinkBufferTooShort { .. } => Self::SinkBufferTooShort,
        }
    }
}

#[derive(Default)]
pub(super) struct FailureMailbox(AtomicU64);

impl FailureMailbox {
    pub(super) fn publish(&self, cause: Failure) {
        self.publish_native(cause, 0);
    }

    pub(super) fn publish_native(&self, cause: Failure, native_code: i32) {
        // The first pending cause stays until control consumes it; terminal readers never clear it.
        // The atomic is the entire payload; no separate memory is published.
        // The low 32 bits identify the cause; the high 32 bits retain native errno.
        let code = u64::from(u32::from_le_bytes(native_code.to_le_bytes()));
        let payload = (code << 32) | u64::from(cause as u32);
        let _ = self
            .0
            .compare_exchange(0, payload, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub(super) fn read(&self) -> Option<Failure> {
        Self::decode(self.0.load(Ordering::Relaxed))
    }

    pub(super) fn take(&self) -> Option<(Failure, i32)> {
        // The complete advisory payload transfers in one atomic operation.
        let payload = self.0.swap(0, Ordering::Relaxed);
        let bytes = payload.to_le_bytes();
        Self::decode(payload).map(|cause| {
            (
                cause,
                i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            )
        })
    }

    fn decode(value: u64) -> Option<Failure> {
        [
            Failure::InvalidFormat,
            Failure::SampleRateMismatch,
            Failure::CapacityExceeded,
            Failure::ClockDiscontinuity,
            Failure::TimestampRegressed,
            Failure::InvalidTransfer,
            Failure::FrameCountExceeded,
            Failure::SourceCountMismatch,
            Failure::SinkCountMismatch,
            Failure::SourceBufferTooShort,
            Failure::SinkBufferTooShort,
            Failure::EndpointSilenceCapacity,
            Failure::ResourceRevision,
            Failure::RetainedCaptures,
            Failure::RetainedDevices,
            Failure::RetainedEndpoints,
            Failure::RetirementPending,
            Failure::OutputReleaseRejected,
        ]
        .into_iter()
        .find(|cause| u64::from(*cause as u32) == value & u64::from(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_terminal_cause_survives_later_callbacks_without_rt_allocation() {
        let mailbox = FailureMailbox::default();
        assert_eq!(mailbox.read(), None);
        let measured = allocation_counter::measure(|| {
            mailbox.publish(ProcessBlockError::InvalidCycle(CycleError::TimestampRegressed).into());
            mailbox.publish(Failure::InvalidTransfer);
        });
        assert_eq!(measured.count_total, 0);
        assert_eq!(mailbox.read(), Some(Failure::TimestampRegressed));
        assert_eq!(mailbox.read(), Some(Failure::TimestampRegressed));
        assert_eq!(mailbox.take(), Some((Failure::TimestampRegressed, 0)));
        assert_eq!(mailbox.read(), None);
        mailbox.publish(Failure::RetainedCaptures);
        assert_eq!(mailbox.take(), Some((Failure::RetainedCaptures, 0)));
        mailbox.publish_native(Failure::OutputReleaseRejected, -32);
        assert_eq!(mailbox.take(), Some((Failure::OutputReleaseRejected, -32)));
    }
}
