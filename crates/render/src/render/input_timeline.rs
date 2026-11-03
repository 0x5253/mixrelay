//! Graph-owned session continuity, independent of adapter owner replacement.
use super::{ExternalBinding, ExternalIo, ProcessBlockError};
use mixrelay_audio_io::process::InputActivity;
use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct State {
    previous: Option<u64>,
}

impl State {
    pub(super) fn read(
        &mut self,
        external: &mut impl ExternalIo,
        binding: &ExternalBinding,
        samples: &mut [f32],
        cycle: &ProcessCycle,
    ) -> Result<TransferOutcome, ProcessBlockError> {
        let before = external.source_epoch(binding);
        let mut transfer = external.read_source(binding, samples, cycle);
        let after = external.source_epoch(binding);
        if transfer.frames > cycle.frames {
            return Err(ProcessBlockError::InvalidTransfer);
        }
        if before.is_none() && after.is_none() {
            return Ok(transfer);
        }
        let epoch = match (before, after) {
            (Some(before), Some(after)) if before == after && after != 0 => after,
            _ => {
                // A retired read may still have copied its old, pinned mapping.
                // Never label those samples with the replacement's epoch.
                samples.fill(0.0);
                transfer.frames = 0;
                transfer.activity = InputActivity::Inactive;
                0
            }
        };
        // Initial admission is priming. Loss resets once, and the first recovery
        // keeps that reset rather than clearing newly mixed tails a second time.
        transfer.discontinuity |= self.previous.is_some_and(|old| old != 0 && old != epoch);
        self.previous = Some(epoch);
        Ok(transfer)
    }
}
