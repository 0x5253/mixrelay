//! Publishes prepared route-delay histories from the control thread to audio owners.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicPtr, Ordering},
};

use parking_lot::Mutex;

use crate::render::{BlockProcessor, DelayBank, RenderPlan};

#[cfg(test)]
mod tests;

struct PreparedSnapshot {
    plan: Arc<RenderPlan>,
    observations: Vec<Option<u32>>,
}

#[derive(Default)]
struct ObservationState {
    plan: Option<Arc<RenderPlan>>,
    prepared: Option<PreparedSnapshot>,
}

/// Transfers derived route-history banks without blocking or destroying them on audio.
///
/// `refresh`, `set_plan`, and `reclaim` are control-thread operations. `activate` is an
/// audio-thread operation and never allocates, locks, or drops a bank. Drop this exchange
/// only from control after every audio caller has quiesced.
pub(crate) struct LatencyExchange {
    pending: AtomicPtr<DelayBank>,
    retired: AtomicPtr<DelayBank>,
    audio_gate: AtomicBool,
    state: Mutex<ObservationState>,
}

impl LatencyExchange {
    /// Creates an empty exchange. No plan is observed until `set_plan` is called.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            pending: AtomicPtr::new(std::ptr::null_mut()),
            retired: AtomicPtr::new(std::ptr::null_mut()),
            audio_gate: AtomicBool::new(false),
            state: Mutex::new(ObservationState::default()),
        }
    }

    /// Records the newest prepared topology; this does not query or prepare processors.
    pub(crate) fn set_plan(&self, plan: Arc<RenderPlan>) {
        let mut state = self.state.lock();
        if state
            .plan
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &plan))
        {
            return;
        }
        state.prepared = None;
        state.plan = Some(plan);
        // Removed owners must not be retained until the next successful allocation. Audio
        // either removed the old pointer already or loses the race to this control-side swap.
        let superseded = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !superseded.is_null() {
            // SAFETY: the swap transfers the slot's sole owner; audio cannot access it now.
            drop(unsafe { Box::from_raw(superseded) });
        }
    }

    /// Control-only immutable topology observation; never borrows the audio processor.
    pub(crate) fn observed_plan(&self) -> Option<Arc<RenderPlan>> {
        self.state.lock().plan.clone()
    }

    /// Prepares a changed observation set and publishes it for a block boundary.
    ///
    /// This is control-thread-only. An unchanged plan and cached observation set returns
    /// without allocating. Preparation errors leave the last prepared snapshot and pending
    /// bank intact so the caller can report and retry.
    ///
    /// # Errors
    /// Returns a bounded route-history preparation or reservation error.
    pub(crate) fn refresh(&self) -> Result<bool, String> {
        let mut state = self.state.lock();
        self.reclaim_retired();
        let Some(plan) = state.plan.clone() else {
            return Ok(false);
        };
        if state.prepared.as_ref().is_some_and(|snapshot| {
            Arc::ptr_eq(&snapshot.plan, &plan) && plan.latency_matches(&snapshot.observations)
        }) {
            return Ok(false);
        }

        let observations = plan.latency_observations();
        let mut cached_observations = Vec::new();
        cached_observations
            .try_reserve_exact(observations.len())
            .map_err(|error| error.to_string())?;
        cached_observations.extend_from_slice(&observations);
        let bank = Box::new(DelayBank::prepare(Arc::clone(&plan), observations)?);
        if !plan.latency_matches(&cached_observations) {
            return Ok(false);
        }

        self.publish_pending(bank);
        state.prepared = Some(PreparedSnapshot {
            plan,
            observations: cached_observations,
        });
        Ok(true)
    }

    /// Reclaims route histories retired by audio, and any bank superseded before activation.
    pub(crate) fn reclaim(&self) {
        let _state = self.state.lock();
        self.reclaim_retired();
    }

    /// Whether a prepared bank is waiting for a compatible audio block boundary.
    #[must_use]
    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.load(Ordering::Acquire).is_null()
    }

    /// Installs a bank prepared for this exact topology without waiting or dropping on audio.
    ///
    /// A bank for a future topology is requeued while that topology is not active. If a newer
    /// bank was published meanwhile, the superseded bank is retired for control-side cleanup.
    pub(crate) fn activate(&self, current: &mut BlockProcessor) -> bool {
        if !self.has_pending() {
            return false;
        }
        let Some(_audio_guard) = AudioGateGuard::try_acquire(&self.audio_gate) else {
            return false;
        };
        if !self.retired.load(Ordering::Acquire).is_null() {
            return false;
        }
        let pending = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if pending.is_null() {
            return false;
        }

        // SAFETY: swapping the pending slot gives this gated audio caller unique ownership;
        // control can no longer replace or drop this pointer.
        let matches_plan = unsafe { Arc::ptr_eq(&(*pending).plan, current.plan()) };
        if !matches_plan {
            // SAFETY: the removed pending owner remains exclusive until requeue/retirement.
            let future_plan = unsafe { (*pending).plan.revision() > current.plan().revision() };
            if !future_plan || !self.requeue_future(pending) {
                self.retired.store(pending, Ordering::Release);
            }
            return false;
        }

        // SAFETY: the pointer was removed from `pending` above and remains uniquely owned.
        let bank = unsafe { Box::from_raw(pending) };
        if let Some(previous) = current.replace_delay_bank(bank) {
            self.retired
                .store(Box::into_raw(previous), Ordering::Release);
        }
        true
    }

    fn requeue_future(&self, pending: *mut DelayBank) -> bool {
        self.pending
            .compare_exchange(
                std::ptr::null_mut(),
                pending,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    fn publish_pending(&self, bank: Box<DelayBank>) {
        let pending = Box::into_raw(bank);
        let superseded = self.pending.swap(pending, Ordering::AcqRel);
        if !superseded.is_null() {
            // SAFETY: the atomic swap transferred the previous pending bank's unique ownership
            // to this control-thread publisher; the audio side can only own a removed pointer.
            unsafe { drop(Box::from_raw(superseded)) };
        }
    }

    fn reclaim_retired(&self) {
        let retired = self.retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !retired.is_null() {
            // SAFETY: the atomic swap transferred the retired bank's unique ownership to this
            // control-thread reclaimer; no audio caller accesses it after publishing it.
            unsafe { drop(Box::from_raw(retired)) };
        }
    }
}

impl Default for LatencyExchange {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LatencyExchange {
    fn drop(&mut self) {
        let pending = self.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pending.is_null() {
            // SAFETY: exclusive `&mut self` and the documented quiescence requirement make this
            // the unique final owner of any bank still in the pending slot.
            unsafe { drop(Box::from_raw(pending)) };
        }
        let retired = self.retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !retired.is_null() {
            // SAFETY: exclusive `&mut self` and the documented quiescence requirement make this
            // the unique final owner of any bank still in the retired slot.
            unsafe { drop(Box::from_raw(retired)) };
        }
    }
}

struct AudioGateGuard<'a> {
    gate: &'a AtomicBool,
}

impl AudioGateGuard<'_> {
    fn try_acquire(gate: &AtomicBool) -> Option<AudioGateGuard<'_>> {
        gate.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| AudioGateGuard { gate })
    }
}

impl Drop for AudioGateGuard<'_> {
    fn drop(&mut self) {
        self.gate.store(false, Ordering::Release);
    }
}
