use super::{PcmRing, Reader};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};

pub(super) struct ReadWindow {
    pub(super) cursor: u64,
    pub(super) epoch: u64,
}

impl PcmRing {
    pub(super) fn reserve_write(&self, writer_epoch: &mut u64, frames: u32) -> Option<u64> {
        let state = &self.publication;
        let written = state.written.load(Relaxed);
        let epoch = state.requested_epoch.load(Acquire);
        if epoch != *writer_epoch {
            // A reader must not combine an older epoch with the next epoch's
            // start. A single bounded snapshot observes both or defers this read.
            state.epoch_sequence.fetch_add(1, SeqCst);
            state.epoch_start.store(written, SeqCst);
            state.published_epoch.store(epoch, SeqCst);
            state.epoch_sequence.fetch_add(1, SeqCst);
            *writer_epoch = epoch;
        }
        // ACK precedes even zero-size/overflow rejection: a full stale queue
        // must let the consumer discard its old epoch without producer progress.
        if frames == 0 {
            return None;
        }
        let read = state.read.load(Acquire);
        if frames > self.capacity / 2
            || written.wrapping_sub(read) > u64::from(self.capacity - frames)
        {
            state.overflows.fetch_add(1, Relaxed);
            return None;
        }
        if frames > state.quantum.load(Relaxed) {
            state.quantum.store(frames, Relaxed);
        }
        Some(written)
    }

    pub(super) fn observed_epoch(&self, after_sequence: impl FnOnce()) -> Option<(u64, u64)> {
        let state = &self.publication;
        let sequence = state.epoch_sequence.load(SeqCst);
        if sequence & 1 != 0 {
            return None;
        }
        // Monomorphized no-op in callbacks; tests advance the actual producer
        // here to exercise an epoch change between the reader's atomic loads.
        after_sequence();
        let epoch = state.published_epoch.load(SeqCst);
        let start = state.epoch_start.load(SeqCst);
        (sequence == state.epoch_sequence.load(SeqCst)
            && epoch == state.requested_epoch.load(Acquire))
        .then_some((epoch, start))
    }

    pub(super) fn reserve_read(
        &self,
        reader: &mut Reader,
        frames: u32,
        prefill: bool,
    ) -> Option<ReadWindow> {
        let Some((epoch, start)) = self.observed_epoch(|| {}) else {
            reader.primed = false;
            return None;
        };
        let state = &self.publication;
        if epoch != reader.epoch {
            let previous = state.read.load(Relaxed);
            // Discard may have already crossed this boundary; never rewind it.
            if start > previous {
                state.read.store(start, Release);
            }
            reader.epoch = epoch;
            reader.primed = false;
        }
        let read = state.read.load(Relaxed);
        let written = state.written.load(Acquire);
        let available = written.wrapping_sub(read);
        let reserve = frames
            .max(state.quantum.load(Relaxed))
            .saturating_mul(2)
            .min(self.capacity);
        if available > u64::from(self.capacity)
            || available < u64::from(frames)
            || (prefill && !reader.primed && available < u64::from(reserve))
        {
            if reader.primed {
                state.starvations.fetch_add(1, Relaxed);
            }
            reader.primed = false;
            return None;
        }
        reader.primed = true;
        Some(ReadWindow {
            cursor: read,
            epoch,
        })
    }

    pub(super) fn finish_read(
        &self,
        reader: &mut Reader,
        window: &ReadWindow,
        frames: u32,
    ) -> bool {
        self.publication
            .read
            .store(window.cursor.wrapping_add(u64::from(frames)), Release);
        if window.epoch != self.publication.requested_epoch.load(Acquire) {
            reader.primed = false;
            return false;
        }
        true
    }

    pub(super) fn spans(&self, cursor: u64, frames: u32) -> (usize, usize, usize) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a remainder modulo the u32 capacity is below that capacity"
        )]
        let start = (cursor % u64::from(self.capacity)) as u32;
        let first = frames.min(self.capacity - start);
        (
            start as usize * self.channels as usize,
            first as usize * self.channels as usize,
            (frames - first) as usize * self.channels as usize,
        )
    }
}
