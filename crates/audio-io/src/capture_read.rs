//! Source-local discontinuity cursor independent of diagnostic report polling.
use crate::process::TransferOutcome;

#[derive(Debug, Default)]
pub struct CaptureRead {
    last_failure: u64,
    read_in_block: bool,
    fresh_admission: bool,
}

impl CaptureRead {
    /// Fences samples queued while this source was unused before its next admitted read.
    pub fn prepare_read(&mut self, discard: impl FnOnce()) {
        if std::mem::take(&mut self.fresh_admission) {
            // Native callbacks may have queued more OFF-period PCM after the
            // last unused block's discard and before this first resumed read.
            discard();
        }
    }

    /// Retains PCM for readers and discards unused source history at a graph boundary.
    pub fn finish_block(&mut self, discard: impl FnOnce()) -> bool {
        let read = std::mem::take(&mut self.read_in_block);
        if !read {
            discard();
            self.fresh_admission = true;
        }
        read
    }

    /// Reads one source and reports loss observed before or during this transfer.
    pub fn read(
        &mut self,
        destination: &mut [f32],
        channels: usize,
        mut failures: impl FnMut() -> u64,
        read: impl FnOnce(&mut [f32]) -> u32,
    ) -> TransferOutcome {
        self.read_in_block = true;
        let before = failures();
        let frames = read(destination);
        let after = failures();
        let discontinuity = before != self.last_failure || after != before;
        self.last_failure = after;
        let source_sample_faults = destination
            .get_mut(..(frames as usize).saturating_mul(channels))
            .map_or(0, crate::pcm::sanitize_samples);
        TransferOutcome {
            discontinuity,
            source_sample_faults,
            ..TransferOutcome::complete(frames)
        }
    }
}

#[cfg(test)]
mod tests;
