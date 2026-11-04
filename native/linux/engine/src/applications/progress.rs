//! Graph-frame progress permission for forwarding a captured application's original audio.

pub(super) struct Progress {
    missing: u64,
    limit: u64,
    delivered: bool,
}

impl Progress {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            missing: 0,
            // Three seconds distinguishes a stopped PCM producer from normal
            // callback batching. Real silence still delivers frames and resets it.
            limit: u64::from(sample_rate) * 3,
            delivered: false,
        }
    }

    pub(super) fn delivered(&mut self, frames: u32) {
        self.delivered |= frames != 0;
    }

    pub(super) fn finish(&mut self, frames: u32, used: bool) -> bool {
        let delivered = std::mem::take(&mut self.delivered);
        if !used || delivered {
            self.missing = 0;
        } else {
            self.missing = self.missing.saturating_add(u64::from(frames));
        }
        used && self.missing < self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_progress_but_three_seconds_without_pcm_revokes_forward_permission() {
        let mut progress = Progress::new(48_000);
        for _ in 0..2_000 {
            progress.delivered(128); // Valid zero PCM is still a complete block.
            assert!(progress.finish(128, true));
        }
        for _ in 0..1_124 {
            progress.delivered(0);
            assert!(progress.finish(128, true));
        }
        assert!(!progress.finish(128, true));
        assert!(!progress.finish(128, true));
        progress.delivered(128);
        assert!(progress.finish(128, true));
        assert_eq!(progress.missing, 0);
    }

    #[test]
    fn off_revokes_immediately_and_any_read_in_the_block_preserves_shared_capture() {
        let mut progress = Progress::new(44_100);
        progress.delivered(16);
        progress.delivered(0); // A second shared reference cannot erase progress.
        assert!(progress.finish(32, true));
        assert!(!progress.finish(32, false));
        assert_eq!(progress.missing, 0);
        progress.delivered(32);
        assert!(progress.finish(32, true));
    }
}
