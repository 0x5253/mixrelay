//! Block-coherent execution gates and the native source/sink transfer contract.
use super::{ControlValue, ExternalBinding, RenderPlan};
use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};

/// Borrowed source/sink gates from exactly the control snapshot used for this audio block.
/// Adapters may invalidate a stopped transport cursor before reading resumed PCM. No bank lookup,
/// independent atomic snapshot, allocation, or lock is required at this boundary.
#[derive(Debug, Clone, Copy)]
pub struct ExternalControlState<'a> {
    plan: &'a RenderPlan,
    controls: &'a [ControlValue],
}

impl<'a> ExternalControlState<'a> {
    pub(super) const fn new(plan: &'a RenderPlan, controls: &'a [ControlValue]) -> Self {
        Self { plan, controls }
    }

    /// Returns the gate for a source-binding index, or `None` outside the prepared layout.
    #[must_use]
    pub fn source_enabled(&self, index: usize) -> Option<bool> {
        self.plan
            .source_controls
            .get(index)
            .map(|&control| self.controls[control].control.enabled)
    }

    /// Returns the gate for a sink-binding index, or `None` outside the prepared layout.
    #[must_use]
    pub fn sink_enabled(&self, index: usize) -> Option<bool> {
        self.plan
            .sink_controls
            .get(index)
            .map(|&control| self.controls[control].control.enabled)
    }

    /// Iterates prepared source identities and their block-coherent execution gates.
    #[must_use]
    pub fn sources(&self) -> impl ExactSizeIterator<Item = (&ExternalBinding, bool)> {
        self.plan
            .source_bindings
            .iter()
            .zip(&self.plan.source_controls)
            .map(|(binding, &index)| (binding, self.controls[index].control.enabled))
    }

    /// Iterates prepared sink identities and their block-coherent execution gates.
    #[must_use]
    pub fn sinks(&self) -> impl ExactSizeIterator<Item = (&ExternalBinding, bool)> {
        self.plan
            .sink_bindings
            .iter()
            .zip(&self.plan.sink_controls)
            .map(|(binding, &index)| (binding, self.controls[index].control.enabled))
    }
}

/// Whether native transport capacity permits this validated quantum to consume PCM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleAdmission {
    /// The adapter has installed its gates and reserved the required transport capacity.
    Process,
    /// Only the pacing owner changed: native PCM queues remained continuous.
    /// Validate the new clock coordinates but retain bus/delay history. Source-local
    /// transfer discontinuities still invalidate their affected histories normally.
    ClockHandoff,
    /// Gates were observed, but transport capacity is unavailable. Retry without advancing time.
    Retry,
}

pub trait ExternalIo {
    /// Observes the same block-boundary gates the processor uses before any external transfer.
    /// Implementations may reset transport cursors on OFF transitions, but must not allocate,
    /// block, or query a separately published control snapshot. Return `Retry` only after gate
    /// changes have been observed, before consuming PCM or running conversion/DSP work.
    fn begin_cycle(
        &mut self,
        _cycle: &ProcessCycle,
        _controls: ExternalControlState<'_>,
    ) -> CycleAdmission {
        CycleAdmission::Process
    }

    /// Optional source-session identity, observed before and after each PCM read.
    /// `None` leaves discontinuity tracking with the adapter. `Some(0)` means
    /// the source is unavailable; nonzero epochs must never be reused. Reads
    /// spanning an epoch change are rejected. This is not a content timestamp.
    /// Implementations must use bounded, nonblocking snapshots only.
    fn source_epoch(&self, _binding: &ExternalBinding) -> Option<u64> {
        None
    }

    /// Native I/O must consume this cycle's timeline, not infer a second clock
    /// from the destination capacity. Never block or allocate at this boundary.
    /// The destination arrives zeroed, so an adapter fills only what it reads and
    /// need not clear it again.
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome;

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome;
}
