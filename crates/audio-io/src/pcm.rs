//! Finite PCM admission and fixed-size accounting before stateful conversion.

use std::sync::atomic::{AtomicU64, Ordering};

/// Replaces NaN and infinities with zero, retaining every finite bit pattern.
/// Returns the number of replaced samples, without allocating or retaining PCM.
#[must_use]
pub fn sanitize_samples(samples: &mut [f32]) -> u64 {
    let mut faults = 0;
    for sample in samples {
        if !sample.is_finite() {
            *sample = 0.0;
            faults += 1;
        }
    }
    faults
}

/// Pending replacements, atomically drained by the graph independently of PCM.
/// This bounded transport count wraps; graph/status lifetime totals saturate.
/// Native queue backpressure bounds unreported samples far below a complete turn.
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct SampleFaultCounter(AtomicU64);

impl SampleFaultCounter {
    pub fn record(&self, samples: u64) {
        if samples != 0 {
            // Only the count crosses threads, so no PCM publication depends on
            // ordering. A racing drain either receives this addition or leaves
            // it pending for the next graph read.
            self.0.fetch_add(samples, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn take(&self) -> u64 {
        // The atomic exchange assigns every pending replacement to one reader.
        self.0.swap(0, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
