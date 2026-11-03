//! Owns relaxed, allocation-free process counters and their control-plane snapshots.

use mixrelay_audio_io::process::{ProcessReport, SampleFaults};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct ProcessTelemetry {
    input_starvations: AtomicU64,
    output_overflows: AtomicU64,
    discontinuities: AtomicU64,
    sample_fault_sources: AtomicU64,
    sample_fault_inputs: AtomicU64,
    sample_fault_outputs: AtomicU64,
    sample_fault_blocks: AtomicU64,
}

impl ProcessTelemetry {
    pub(super) fn snapshot(&self) -> ProcessReport {
        ProcessReport {
            input_starvations: self.input_starvations.load(Ordering::Relaxed),
            output_overflows: self.output_overflows.load(Ordering::Relaxed),
            discontinuities: self.discontinuities.load(Ordering::Relaxed),
            sample_faults: SampleFaults {
                source_samples: self.sample_fault_sources.load(Ordering::Relaxed),
                input_samples: self.sample_fault_inputs.load(Ordering::Relaxed),
                output_samples: self.sample_fault_outputs.load(Ordering::Relaxed),
                blocks: self.sample_fault_blocks.load(Ordering::Relaxed),
            },
        }
    }

    pub(super) fn record(&self, report: mixrelay_audio_io::process::ProcessReport) {
        self.discontinuities
            .fetch_add(report.discontinuities, Ordering::Relaxed);
        self.input_starvations
            .fetch_add(report.input_starvations, Ordering::Relaxed);
        self.output_overflows
            .fetch_add(report.output_overflows, Ordering::Relaxed);
        for (counter, increment) in [
            (
                &self.sample_fault_sources,
                report.sample_faults.source_samples,
            ),
            (
                &self.sample_fault_inputs,
                report.sample_faults.input_samples,
            ),
            (
                &self.sample_fault_outputs,
                report.sample_faults.output_samples,
            ),
            (&self.sample_fault_blocks, report.sample_faults.blocks),
        ] {
            if increment != 0 {
                let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_add(increment))
                });
            }
        }
    }
}
