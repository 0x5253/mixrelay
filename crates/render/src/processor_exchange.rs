//! Block-boundary topology publication and deferred processor reclamation.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
};

use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    BlockProcessor, RenderPlan, RuntimeControlBank, RuntimeControlError, RuntimeControlUpdate,
    latency_exchange, runtime_control,
};

/// Transfers prepared topologies from control to the sole audio owner at block boundaries.
///
/// Control reclaims retired processors after ownership transfer. The exchange itself must be
/// dropped on control after audio callbacks have quiesced, since processor destruction can stop
/// hosted workers and release native resources.
pub struct ProcessorExchange {
    gate: AtomicBool,
    pending: AtomicPtr<BlockProcessor>,
    retired: AtomicPtr<BlockProcessor>,
    active_revision: AtomicU64,
    submitted_revision: AtomicU64,
    control_revisions: Arc<runtime_control::ControlRevisionClock>,
    control_writer: Mutex<()>,
    latency: latency_exchange::LatencyExchange,
}

struct ExchangeGateGuard<'a> {
    gate: &'a AtomicBool,
}

impl Drop for ExchangeGateGuard<'_> {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}

impl ProcessorExchange {
    #[must_use]
    pub fn new(active_revision: u64) -> Self {
        Self {
            gate: AtomicBool::new(false),
            pending: AtomicPtr::new(std::ptr::null_mut()),
            retired: AtomicPtr::new(std::ptr::null_mut()),
            active_revision: AtomicU64::new(active_revision),
            submitted_revision: AtomicU64::new(active_revision),
            control_revisions: Arc::default(),
            control_writer: Mutex::new(()),
            latency: latency_exchange::LatencyExchange::default(),
        }
    }

    /// Submits a fully prepared processor from the control thread.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessorSubmitError::Busy`] when another submission or block-boundary
    /// activation owns the non-blocking exchange gate, or a processor is already pending.
    /// Rejects stale revisions or a control bank already owned by another exchange.
    pub fn submit(&self, processor: Box<BlockProcessor>) -> Result<(), ProcessorSubmitError> {
        self.submit_configured(processor, || Ok::<(), std::convert::Infallible>(()))
            .map_err(|error| match error {
                ProcessorConfigurationError::Busy { proposed_revision } => {
                    ProcessorSubmitError::Busy { proposed_revision }
                }
                ProcessorConfigurationError::WrongExchange { proposed_revision } => {
                    ProcessorSubmitError::WrongExchange { proposed_revision }
                }
                ProcessorConfigurationError::StaleRevision { current, proposed } => {
                    ProcessorSubmitError::StaleRevision { current, proposed }
                }
                ProcessorConfigurationError::Configuration(error) => match error {},
            })
    }

    /// Runs a control-plane configuration step and publishes a prepared processor as one
    /// submission. The audio thread never waits for the configuration step: it keeps using the
    /// active processor when the exchange gate is occupied.
    ///
    /// # Errors
    ///
    /// Returns [`ProcessorConfigurationError::Busy`] when a processor cannot be queued, or
    /// [`ProcessorConfigurationError::Configuration`] when `configure` rejects the associated
    /// control-plane state. Stale revisions and banks from another exchange are rejected before
    /// the configuration closure is called.
    pub fn submit_configured<E>(
        &self,
        processor: Box<BlockProcessor>,
        configure: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), ProcessorConfigurationError<E>> {
        let _control_writer = self.control_writer.lock();
        let proposed_revision = processor.plan().revision();
        let current = self.revisions().submitted;
        if proposed_revision <= current {
            return Err(ProcessorConfigurationError::StaleRevision {
                current,
                proposed: proposed_revision,
            });
        }
        if self
            .gate
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(ProcessorConfigurationError::Busy { proposed_revision });
        }
        let gate_guard = ExchangeGateGuard { gate: &self.gate };
        self.reclaim_retired();
        if !self.pending.load(Ordering::Acquire).is_null() {
            return Err(ProcessorConfigurationError::Busy { proposed_revision });
        }
        processor
            .plan()
            .controls()
            .bind_clock(&self.control_revisions)
            .map_err(|_| ProcessorConfigurationError::WrongExchange { proposed_revision })?;
        if let Err(error) = configure() {
            return Err(ProcessorConfigurationError::Configuration(error));
        }
        self.latency.set_plan(Arc::clone(processor.plan()));
        self.pending
            .store(Box::into_raw(processor), Ordering::Release);
        self.submitted_revision
            .store(proposed_revision, Ordering::Release);
        drop(gate_guard);
        Ok(())
    }

    /// Activates a pending processor at an audio block boundary without allocation,
    /// deallocation, blocking, or waiting.
    pub fn activate_pending(&self, current: &mut Box<BlockProcessor>) -> bool {
        self.activate_pending_configured(current, |_| Ok::<(), std::convert::Infallible>(()))
            .unwrap_or_else(|error| match error {})
    }

    /// Control-side hint for callback-owner reconfiguration. Activation still
    /// goes through the serialized exchange; this does not consume a plan.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        !self.pending.load(Ordering::Acquire).is_null()
    }

    /// Includes only derived route-history publication, not a native topology change.
    #[must_use]
    pub fn has_pending_latency(&self) -> bool {
        self.latency.has_pending()
    }

    /// Registers the initial prepared plan before the sole audio owner starts.
    /// Later plans are registered atomically with successful topology submission.
    pub fn observe_plan(&self, plan: Arc<RenderPlan>) {
        self.latency.set_plan(plan);
    }

    /// Returns the observed topology only after its audio owner acknowledged activation.
    /// Control-thread-only; pending topology timing must never be reported as applied.
    #[must_use]
    pub fn latency_plan(&self) -> Option<Arc<RenderPlan>> {
        self.latency
            .observed_plan()
            .filter(|plan| plan.revision() == self.plan_revisions().active)
    }

    /// Refreshes cached worker delay observations on a control/background thread.
    /// No native processor preparation or graph/control revision change occurs.
    /// # Errors
    /// Returns a preparation diagnostic when bounded route storage cannot be allocated.
    pub fn refresh_latency(&self) -> Result<bool, String> {
        self.latency.refresh()
    }

    /// Activates a pending processor and its pre-staged external configuration at one block
    /// boundary. `activate` must perform only bounded, real-time-safe work. The active revision is
    /// published only after both swaps succeed.
    ///
    /// # Errors
    ///
    /// Returns the error from `activate` without replacing the current processor. The pending
    /// processor remains queued for a later activation attempt.
    pub fn activate_pending_configured<E>(
        &self,
        current: &mut Box<BlockProcessor>,
        activate: impl FnOnce(u64) -> Result<(), E>,
    ) -> Result<bool, E> {
        self.activate_pending_if_configured(current, |_| true, activate)
    }

    /// Activates only a prepared topology compatible with the current callback owner.
    /// Both closures must be bounded and real-time safe. An incompatible topology stays pending
    /// for the control thread to rebind external resources without stopping compatible edits.
    ///
    /// # Errors
    /// Returns the external activation error and leaves the current processor untouched.
    pub fn activate_pending_if_configured<E>(
        &self,
        current: &mut Box<BlockProcessor>,
        compatible: impl FnOnce(&RenderPlan) -> bool,
        activate: impl FnOnce(u64) -> Result<(), E>,
    ) -> Result<bool, E> {
        // Idle callbacks must not reserve the publication gate and spuriously
        // reject a control writer. A concurrent publication missed by this
        // acquire is simply observed at the next block; the gated swap below
        // remains the sole owner transfer and rechecks the pending slot.
        if !self.has_pending() {
            self.latency.activate(current);
            return Ok(false);
        }
        if self
            .gate
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Ok(false);
        }
        let gate_guard = ExchangeGateGuard { gate: &self.gate };
        let pending = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            return Ok(false);
        }
        // SAFETY: only `submit` creates pending pointers and the exchange gate grants
        // this audio thread unique ownership after removing the pointer from the slot.
        let mut next = unsafe { Box::from_raw(pending) };
        if !compatible(next.plan()) || !next.can_adopt_hosted(current) {
            self.pending.store(Box::into_raw(next), Ordering::Release);
            return Ok(false);
        }
        let revision = next.plan().revision();
        if let Err(error) = activate(revision) {
            self.pending.store(Box::into_raw(next), Ordering::Release);
            return Err(error);
        }
        current.set_hosted_active(false);
        next.adopt_hosted(current);
        next.retain_history(current);
        next.retain_device_bus_state(current);
        next.set_hosted_active(true);
        let previous = std::mem::replace(current, next);
        debug_assert!(self.retired.load(Ordering::Relaxed).is_null());
        self.retired
            .store(Box::into_raw(previous), Ordering::Release);
        self.active_revision.store(revision, Ordering::Release);
        self.latency.activate(current);
        drop(gate_guard);
        Ok(true)
    }

    #[must_use]
    pub fn revisions(&self) -> ProcessorRevisions {
        let plans = self.plan_revisions();
        let active = plans
            .active
            .max(self.control_revisions.applied.load(Ordering::Acquire));
        ProcessorRevisions {
            active,
            submitted: plans
                .submitted
                .max(active)
                .max(self.control_revisions.published.load(Ordering::Acquire)),
        }
    }

    /// Revisions of prepared topologies only, for native attachments and meter layouts.
    #[must_use]
    pub fn plan_revisions(&self) -> ProcessorRevisions {
        ProcessorRevisions {
            active: self.active_revision.load(Ordering::Acquire),
            submitted: self.submitted_revision.load(Ordering::Acquire),
        }
    }

    /// Publishes graph-local controls without preparing or exchanging an execution plan.
    /// The bank must belong to the most recently submitted topology. The audio reader acknowledges
    /// its complete batch at a block boundary; publication never calls hosted/native processors.
    ///
    /// # Errors
    /// Rejects stale topology handles, invalid targets/values, or non-increasing revisions.
    pub fn publish_controls(
        &self,
        bank: &RuntimeControlBank,
        revision: u64,
        updates: &[RuntimeControlUpdate],
    ) -> Result<(), RuntimeControlError> {
        let _writer = self.control_writer.lock();
        let current = self.plan_revisions().submitted;
        if bank.plan_revision() != current {
            return Err(RuntimeControlError::StalePlan {
                current,
                proposed: bank.plan_revision(),
            });
        }
        let current = self.revisions().submitted;
        if revision <= current {
            return Err(RuntimeControlError::StaleRevision {
                current,
                proposed: revision,
            });
        }
        bank.bind_clock(&self.control_revisions)?;
        bank.publish(revision, updates)
    }

    /// Reclaims the processor retired by the audio thread.
    ///
    /// This must run on a control/background thread because dropping a hosted processor may stop
    /// a child process and release native plug-in resources. Calling it periodically prevents a
    /// removed plug-in from remaining alive until the next graph edit.
    pub fn reclaim_retired(&self) {
        self.latency.reclaim();
        let retired = self.retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !retired.is_null() {
            // SAFETY: the atomic swap takes the slot's sole owner. Audio only publishes
            // retired processors and cannot access this pointer after its transfer.
            drop(unsafe { Box::from_raw(retired) });
        }
    }
}

impl Drop for ProcessorExchange {
    fn drop(&mut self) {
        for pointer in [*self.pending.get_mut(), *self.retired.get_mut()] {
            if !pointer.is_null() {
                // SAFETY: dropping the exchange requires exclusive access, so no thread can
                // access pointers that remain owned by its pending and retired slots.
                drop(unsafe { Box::from_raw(pointer) });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ProcessorSubmitError {
    #[error(
        "processor revision {proposed_revision} could not be queued because the exchange is busy"
    )]
    Busy { proposed_revision: u64 },
    /// Prepared control storage cannot be shared across independent execution owners.
    #[error("processor revision {proposed_revision} belongs to another exchange")]
    WrongExchange { proposed_revision: u64 },
    /// Topology revisions must follow all accepted topology and control publications.
    #[error("processor revision {proposed} is not newer than {current}")]
    StaleRevision { current: u64, proposed: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProcessorConfigurationError<E> {
    #[error(
        "processor revision {proposed_revision} could not be queued because the exchange is busy"
    )]
    Busy { proposed_revision: u64 },
    /// The proposed bank belongs to another exchange; configuration has not run.
    #[error("processor revision {proposed_revision} belongs to another exchange")]
    WrongExchange { proposed_revision: u64 },
    /// A stale topology cannot run its associated configuration side effects.
    #[error("processor revision {proposed} is not newer than {current}")]
    StaleRevision { current: u64, proposed: u64 },
    #[error("processor control-plane configuration failed: {0}")]
    Configuration(E),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorRevisions {
    pub active: u64,
    pub submitted: u64,
}

#[cfg(test)]
mod tests;
