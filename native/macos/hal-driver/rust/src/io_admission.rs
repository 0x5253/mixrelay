//! Bounded counters for SDK requests rejected before native PCM processing.
use std::sync::atomic::{AtomicU64, Ordering};

// Wire order is shared with MRProbeHalTrace. Counts are independent observations;
// a snapshot taken during IO can include requests whose completion is still pending.
pub(crate) const COUNTERS: usize = 8;

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum Stage {
    Read,
    Write,
    Timestamp,
    Extent,
    Stream,
    Frozen,
    Inactive,
    Native,
}

pub(crate) struct Admission {
    counts: [AtomicU64; COUNTERS],
    rejected_timestamp: AtomicU64,
    rejected_extent: AtomicU64,
    null_buffers: AtomicU64,
    oversized_frames: AtomicU64,
}

impl Admission {
    pub(crate) fn new() -> Self {
        Self {
            counts: std::array::from_fn(|_| AtomicU64::new(0)),
            rejected_timestamp: AtomicU64::new(0),
            rejected_extent: AtomicU64::new(0),
            null_buffers: AtomicU64::new(0),
            oversized_frames: AtomicU64::new(0),
        }
    }

    pub(crate) fn reset(&self) {
        for count in &self.counts {
            count.store(0, Ordering::Relaxed);
        }
        self.rejected_timestamp.store(0, Ordering::Relaxed);
        self.rejected_extent.store(0, Ordering::Relaxed);
        self.null_buffers.store(0, Ordering::Relaxed);
        self.oversized_frames.store(0, Ordering::Relaxed);
    }

    pub(crate) fn note(&self, stage: Stage) {
        // Relaxed counts carry no PCM or lifetime publication. Concurrent snapshots
        // are conservative observations, not a transaction across all categories.
        self.counts[stage as usize].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn timestamp(&self, sample_time: f64) {
        self.rejected_timestamp
            .store(sample_time.to_bits(), Ordering::Relaxed);
        self.note(Stage::Timestamp);
    }

    pub(crate) fn extent(&self, frames: u32, buffer_present: bool, render: bool) {
        let category = if buffer_present {
            &self.oversized_frames
        } else {
            &self.null_buffers
        };
        category.fetch_add(1, Ordering::Relaxed);
        // One atomic word retains a consistent size/pointer/direction diagnostic.
        let value =
            u64::from(frames) | (u64::from(buffer_present) << 32) | (u64::from(render) << 33);
        self.rejected_extent.store(value, Ordering::Relaxed);
        self.note(Stage::Extent);
    }

    pub(crate) fn snapshot(&self) -> [u64; COUNTERS + 4] {
        let mut words = [0; COUNTERS + 4];
        for (word, count) in words.iter_mut().zip(&self.counts) {
            *word = count.load(Ordering::Relaxed);
        }
        words[COUNTERS] = self.rejected_timestamp.load(Ordering::Relaxed);
        words[COUNTERS + 1] = self.rejected_extent.load(Ordering::Relaxed);
        words[COUNTERS + 2] = self.null_buffers.load(Ordering::Relaxed);
        words[COUNTERS + 3] = self.oversized_frames.load(Ordering::Relaxed);
        words
    }
}
