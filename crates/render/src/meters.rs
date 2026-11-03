//! Audio-owned meter publication and control-thread peak reduction.
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[derive(Debug)]
struct PeakMeterCell {
    peak_bits: AtomicU32,
    revision: AtomicU64,
}

#[derive(Debug)]
struct DynamicsMeterCell {
    minimum_gain_bits: AtomicU32,
    threshold_bits: AtomicU32,
    revision: AtomicU64,
    observed: AtomicBool,
}

/// Actual detector gain, before compressor makeup, and the DSP's prepared threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicsMeterReading {
    pub input_peaks: Vec<f32>,
    pub gain_reduction_db: f32,
    pub threshold_db: f32,
}

#[derive(Clone, Copy)]
pub(crate) struct DynamicsMeterBlock {
    pub(crate) channels: u16,
    pub(crate) minimum_gain: f32,
    pub(crate) threshold_db: f32,
}

#[derive(Debug)]
pub struct PeakMeterBank {
    cells: Box<[PeakMeterCell]>,
    input_cells: Box<[PeakMeterCell]>,
    dynamics: Box<[DynamicsMeterCell]>,
    maximum_bindings: usize,
    maximum_channels: usize,
    publication_sequence: AtomicU64,
    completed_revision: AtomicU64,
    completed: AtomicBool,
}

/// A stable completed meter write; odd sequences are still being written by audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeterPublication {
    pub sequence: u64,
    pub revision: u64,
}

impl PeakMeterBank {
    #[must_use]
    pub fn new(maximum_bindings: usize, maximum_channels: usize) -> Self {
        let cell_count = maximum_bindings.saturating_mul(maximum_channels);
        let make_cells = || {
            (0..cell_count)
                .map(|_| PeakMeterCell {
                    peak_bits: AtomicU32::new(0),
                    revision: AtomicU64::new(0),
                })
                .collect()
        };
        Self {
            cells: make_cells(),
            input_cells: make_cells(),
            dynamics: (0..maximum_bindings)
                .map(|_| DynamicsMeterCell {
                    minimum_gain_bits: AtomicU32::new(1.0_f32.to_bits()),
                    threshold_bits: AtomicU32::new(0),
                    revision: AtomicU64::new(0),
                    observed: AtomicBool::new(false),
                })
                .collect(),
            maximum_bindings,
            maximum_channels,
            publication_sequence: AtomicU64::new(0),
            completed_revision: AtomicU64::new(0),
            completed: AtomicBool::new(false),
        }
    }

    /// The service's sole publisher uses this fence around its control-thread drain.
    /// A separate observed bit distinguishes startup from the counter wrapping to zero.
    #[must_use]
    pub fn publication(&self) -> Option<MeterPublication> {
        let sequence = self.publication_sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 || !self.completed.load(Ordering::Acquire) {
            return None;
        }
        let revision = self.completed_revision.load(Ordering::Acquire);
        (self.publication_sequence.load(Ordering::Acquire) == sequence)
            .then_some(MeterPublication { sequence, revision })
    }

    /// A rejected drain may have consumed an in-progress block. Wait beyond its
    /// next even sequence, rather than publishing its partially drained zeros.
    #[must_use]
    pub fn drain_rejection_floor(&self) -> u64 {
        let sequence = self.publication_sequence.load(Ordering::Acquire);
        sequence.wrapping_add(sequence & 1)
    }

    // Meter value writes use Release: an acquiring drain that sees any new value
    // also observes this begin fence, so its final sequence check cannot accept
    // that value under the previous completed publication (including on ARM).
    pub(crate) fn begin_publication(&self) {
        self.publication_sequence.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn finish_publication(&self, revision: u64) {
        self.completed_revision.store(revision, Ordering::Release);
        self.completed.store(true, Ordering::Release);
        self.publication_sequence.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn record_dynamics(
        &self,
        revision: u64,
        binding_index: usize,
        samples: &[f32],
        block: DynamicsMeterBlock,
    ) {
        let DynamicsMeterBlock {
            channels,
            minimum_gain,
            threshold_db,
        } = block;
        let Some(dynamics) = self.dynamics.get(binding_index) else {
            return;
        };
        if !dynamics.observed.load(Ordering::Relaxed)
            || dynamics.revision.load(Ordering::Relaxed) != revision
        {
            dynamics
                .minimum_gain_bits
                .store(1.0_f32.to_bits(), Ordering::Release);
            dynamics
                .threshold_bits
                .store(threshold_db.to_bits(), Ordering::Release);
            dynamics.revision.store(revision, Ordering::Release);
        }
        dynamics
            .minimum_gain_bits
            .fetch_min(minimum_gain.clamp(0.0, 1.0).to_bits(), Ordering::Release);
        let channel_count = usize::from(channels).min(self.maximum_channels);
        for_each_interleaved_peak(samples, channels, channel_count, |channel, peak| {
            if let Some(cell) = self.input_cell(binding_index, channel) {
                if cell.revision.load(Ordering::Relaxed) != revision {
                    cell.peak_bits.store(0, Ordering::Release);
                    cell.revision.store(revision, Ordering::Release);
                }
                cell.peak_bits.fetch_max(peak.to_bits(), Ordering::Release);
            }
        });
        dynamics.observed.store(true, Ordering::Release);
    }

    /// Drained by the control thread; this allocates only while serializing telemetry.
    #[must_use]
    pub fn take_dynamics(
        &self,
        revision: u64,
        binding_index: usize,
        channels: u16,
    ) -> Option<DynamicsMeterReading> {
        let dynamics = self.dynamics.get(binding_index)?;
        if !dynamics.observed.load(Ordering::Acquire)
            || dynamics.revision.load(Ordering::Acquire) != revision
        {
            return None;
        }
        let gain = f32::from_bits(
            dynamics
                .minimum_gain_bits
                .swap(1.0_f32.to_bits(), Ordering::AcqRel),
        );
        Some(DynamicsMeterReading {
            input_peaks: (0..usize::from(channels).min(self.maximum_channels))
                .map(|channel| {
                    let Some(cell) = self.input_cell(binding_index, channel) else {
                        return 0.0;
                    };
                    if cell.revision.load(Ordering::Acquire) != revision {
                        return 0.0;
                    }
                    f32::from_bits(cell.peak_bits.swap(0, Ordering::AcqRel))
                })
                .collect(),
            gain_reduction_db: if gain > 0.0 {
                (-20.0 * gain.log10()).clamp(0.0, 120.0)
            } else {
                120.0
            },
            threshold_db: f32::from_bits(dynamics.threshold_bits.load(Ordering::Relaxed)),
        })
    }

    pub(crate) fn record_interleaved(
        &self,
        revision: u64,
        binding_index: usize,
        channels: u16,
        samples: &[f32],
    ) {
        if binding_index >= self.maximum_bindings {
            return;
        }
        let channel_count = usize::from(channels).min(self.maximum_channels);
        for_each_interleaved_peak(samples, channels, channel_count, |channel, peak| {
            self.record_peak(revision, binding_index, channel, peak);
        });
    }

    fn record_peak(&self, revision: u64, binding_index: usize, channel: usize, peak: f32) {
        let Some(cell) = self.cell(binding_index, channel) else {
            return;
        };
        if cell.revision.load(Ordering::Relaxed) != revision {
            cell.peak_bits.store(0, Ordering::Release);
            cell.revision.store(revision, Ordering::Release);
        }
        cell.peak_bits.fetch_max(peak.to_bits(), Ordering::Release);
    }

    #[must_use]
    pub fn take_peaks(&self, revision: u64, binding_index: usize, channels: u16) -> Vec<f32> {
        (0..usize::from(channels).min(self.maximum_channels))
            .map(|channel| {
                let Some(cell) = self.cell(binding_index, channel) else {
                    return 0.0;
                };
                if cell.revision.load(Ordering::Acquire) != revision {
                    return 0.0;
                }
                f32::from_bits(cell.peak_bits.swap(0, Ordering::AcqRel))
            })
            .collect()
    }

    fn cell(&self, binding_index: usize, channel: usize) -> Option<&PeakMeterCell> {
        binding_index
            .checked_mul(self.maximum_channels)
            .and_then(|offset| offset.checked_add(channel))
            .and_then(|index| self.cells.get(index))
    }

    fn input_cell(&self, binding_index: usize, channel: usize) -> Option<&PeakMeterCell> {
        binding_index
            .checked_mul(self.maximum_channels)
            .and_then(|offset| offset.checked_add(channel))
            .and_then(|index| self.input_cells.get(index))
    }
}

/// Channels whose block peaks are reduced together in one frame-major pass.
const PEAK_GROUP: usize = 64;

/// Reports each of the first `channel_count` channels' finite block peak once. Interleaved
/// PCM is read in memory order, a frame at a time, so the scan stays sequential and can be
/// vectorized; wider ports are reduced in groups of [`PEAK_GROUP`] channels.
fn for_each_interleaved_peak(
    samples: &[f32],
    channels: u16,
    channel_count: usize,
    mut publish: impl FnMut(usize, f32),
) {
    let stride = usize::from(channels);
    let mut peaks = [0.0_f32; PEAK_GROUP];
    for start in (0..channel_count).step_by(PEAK_GROUP) {
        let width = (channel_count - start).min(PEAK_GROUP);
        let group = &mut peaks[..width];
        group.fill(0.0);
        for frame in samples.chunks_exact(stride) {
            for (peak, &value) in group.iter_mut().zip(&frame[start..start + width]) {
                let magnitude = value.abs();
                // Non-finite samples are faults, not levels; they never raise a meter.
                *peak = peak.max(if magnitude.is_finite() {
                    magnitude
                } else {
                    0.0
                });
            }
        }
        for (offset, &peak) in group.iter().enumerate() {
            publish(start + offset, peak);
        }
    }
}

#[cfg(test)]
#[path = "meter_publication_tests.rs"]
mod tests;
