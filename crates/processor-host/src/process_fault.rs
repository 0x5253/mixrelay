//! Fixed native failure counters, observed and reported by the parent monitor outside DSP.

use std::sync::atomic::{AtomicU64, Ordering};
mod native;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FaultKind {
    NativeCall,
    ProcessStatus,
    NonFinitePcm,
    InvalidGeometry,
    ParameterApplication,
    OutputDeadlineMissed,
    InputBackpressure,
}

impl FaultKind {
    const ALL: [Self; 7] = [
        Self::NativeCall,
        Self::ProcessStatus,
        Self::NonFinitePcm,
        Self::InvalidGeometry,
        Self::ParameterApplication,
        Self::OutputDeadlineMissed,
        Self::InputBackpressure,
    ];

    pub(super) const fn cause(self) -> &'static str {
        match self {
            Self::NativeCall => "native_call_failed",
            Self::ProcessStatus => "process_status_error",
            Self::NonFinitePcm => "nonfinite_pcm",
            Self::InvalidGeometry => "invalid_geometry",
            Self::ParameterApplication => "parameter_application_failed",
            Self::OutputDeadlineMissed => "output_deadline_missed",
            Self::InputBackpressure => "input_mailbox_busy",
        }
    }

    pub(super) const fn phase(self) -> &'static str {
        match self {
            Self::ParameterApplication => "parameter",
            Self::OutputDeadlineMissed => "receive",
            Self::InputBackpressure => "submit",
            _ => "process",
        }
    }
}

#[repr(C)]
pub(super) struct ProcessFaults {
    counts: [AtomicU64; FaultKind::ALL.len()],
    native: native::FirstFailure,
}

impl ProcessFaults {
    pub(super) fn new() -> Self {
        Self {
            counts: std::array::from_fn(|_| AtomicU64::new(0)),
            native: native::FirstFailure::new(),
        }
    }

    pub(super) fn record_native(&self, error: &truce_rack_core::Error) {
        self.native.record(error);
    }

    pub(super) fn record(&self, kind: FaultKind) {
        // Counters publish no PCM or ownership; each independent diagnostic needs only atomicity.
        self.counts[kind as usize].fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn total(&self) -> u64 {
        self.counts.iter().fold(0_u64, |total, counter| {
            total.saturating_add(counter.load(Ordering::Relaxed))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FaultDelta {
    pub(super) kind: FaultKind,
    pub(super) new_failures: u64,
    pub(super) total: u64,
}

/// One monitor retains the last reported counts for exactly one worker incarnation.
#[derive(Default)]
pub(super) struct FaultObserver {
    reported: [u64; FaultKind::ALL.len()],
    native_reported: bool,
}

impl FaultObserver {
    pub(super) fn native_failure(
        &mut self,
        faults: &ProcessFaults,
    ) -> Option<truce_rack_core::ProcessFault> {
        if self.native_reported {
            return None;
        }
        let fault = faults.native.read()?;
        self.native_reported = true;
        Some(fault)
    }

    pub(super) fn observe(
        &mut self,
        faults: &ProcessFaults,
    ) -> [Option<FaultDelta>; FaultKind::ALL.len()] {
        std::array::from_fn(|index| {
            // Independent diagnostic counters do not guard any shared audio or control memory.
            let total = faults.counts[index].load(Ordering::Relaxed);
            let new_failures = total.wrapping_sub(self.reported[index]);
            self.reported[index] = total;
            (new_failures != 0).then_some(FaultDelta {
                kind: FaultKind::ALL[index],
                new_failures,
                total,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_reports_each_kind_once_and_preserves_later_deltas() {
        let faults = ProcessFaults::new();
        let mut observer = FaultObserver::default();
        for kind in FaultKind::ALL {
            faults.record(kind);
            faults.record(kind);
        }
        let deltas = observer.observe(&faults);
        for (entry, kind) in deltas.into_iter().zip(FaultKind::ALL) {
            assert_eq!(
                entry,
                Some(FaultDelta {
                    kind,
                    new_failures: 2,
                    total: 2,
                })
            );
        }
        for _ in 0..32 {
            assert!(observer.observe(&faults).iter().all(Option::is_none));
        }
        faults.record(FaultKind::NonFinitePcm);
        assert_eq!(
            observer
                .observe(&faults)
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            [FaultDelta {
                kind: FaultKind::NonFinitePcm,
                new_failures: 1,
                total: 3,
            }]
        );
        assert_eq!(faults.total(), 15);
    }
}
