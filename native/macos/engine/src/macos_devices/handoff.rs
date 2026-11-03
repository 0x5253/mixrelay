//! Resource handoff has two activation boundaries. Geometry changes accompany
//! a DSP plan; same-spec recovery may activate on any block. Format preparation
//! reserves both lanes until its token is committed or cancelled.
use mixrelay_audio_io::recovery::{RecoveryConsumer, RecoveryMailbox};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

const IDLE: u8 = 0;
const FORMAT: u8 = 3;

#[derive(Clone, Copy)]
pub(super) enum Publication {
    Graph = 1,
    Recovery = 2,
}

pub(super) struct ResourceHandoff<T: Send> {
    graph: RecoveryMailbox<T>,
    recovery: RecoveryMailbox<T>,
    state: Arc<AtomicU8>,
    completed: AtomicBool,
}

impl<T: Send> Default for ResourceHandoff<T> {
    fn default() -> Self {
        Self {
            graph: RecoveryMailbox::default(),
            recovery: RecoveryMailbox::default(),
            state: Arc::new(AtomicU8::new(IDLE)),
            completed: AtomicBool::new(false),
        }
    }
}

pub(super) struct FormatReservation(Arc<AtomicU8>);
impl Drop for FormatReservation {
    fn drop(&mut self) {
        let _ = self
            .0
            .compare_exchange(FORMAT, IDLE, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl<T: Send> ResourceHandoff<T> {
    pub(super) fn publish_reserved(&self, token: &FormatReservation, value: T) {
        assert!(
            Arc::ptr_eq(&self.state, &token.0),
            "reservation belongs to this handoff"
        );
        assert_eq!(self.state.load(Ordering::Acquire), FORMAT);
        assert!(
            self.graph.publish(0, value).is_ok(),
            "reserved graph mailbox is empty"
        );
        self.state
            .store(Publication::Graph as u8, Ordering::Release);
    }

    pub(super) fn try_consumer(&self) -> Option<ResourceConsumer<'_, T>> {
        Some(ResourceConsumer {
            graph: self.graph.try_consumer()?,
            recovery: self.recovery.try_consumer()?,
            completed: &self.completed,
        })
    }

    pub(super) fn idle(&self) -> bool {
        self.state.load(Ordering::Acquire) == IDLE
    }

    pub(super) fn has_pending(&self) -> bool {
        matches!(self.state.load(Ordering::Acquire), 1 | 2)
            && !self.completed.load(Ordering::Acquire)
    }

    pub(super) fn reserve_format(&self) -> Option<FormatReservation> {
        self.state
            .compare_exchange(IDLE, FORMAT, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| FormatReservation(self.state.clone()))
    }

    /// Control mutations are serialized by the device cache lock. Reserve
    /// before publication, so the consumer's pending-to-retired transfer cannot
    /// transiently look idle to a concurrent format request.
    pub(super) fn publish(&self, lane: Publication, value: T) -> Result<(), T> {
        if self
            .state
            .compare_exchange(IDLE, lane as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(value);
        }
        if let Err(value) = self.mailbox(lane).publish(0, value) {
            self.state.store(IDLE, Ordering::Release);
            return Err(value);
        }
        Ok(())
    }

    fn mailbox(&self, lane: Publication) -> &RecoveryMailbox<T> {
        match lane {
            Publication::Graph => &self.graph,
            Publication::Recovery => &self.recovery,
        }
    }

    /// Control thread, under the cache lock. Never destroy displaced owners on DSP.
    pub(super) fn reclaim(&self) {
        if self.completed.swap(false, Ordering::AcqRel) {
            self.graph.reclaim();
            self.recovery.reclaim();
            self.state.store(IDLE, Ordering::Release);
        }
    }
}

/// One worker owns both lanes; graph activation still waits for its matching plan.
pub(super) struct ResourceConsumer<'a, T: Send> {
    graph: RecoveryConsumer<'a, T>,
    recovery: RecoveryConsumer<'a, T>,
    completed: &'a AtomicBool,
}

impl<T: Send> ResourceConsumer<'_, T> {
    pub(super) fn activate(&mut self, lane: Publication, active: &mut T) {
        let consumer = match lane {
            Publication::Graph => &mut self.graph,
            Publication::Recovery => &mut self.recovery,
        };
        let mut swapped = false;
        consumer.activate(0, |next| {
            std::mem::swap(active, next);
            swapped = true;
        });
        if swapped {
            // The infallible swap returned and retirement is published before
            // control may reclaim either lane and release its reservation.
            self.completed.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_activates_without_a_graph_revision_but_graph_changes_wait() {
        let handoff = ResourceHandoff::default();
        let mut consumer = handoff.try_consumer().unwrap();
        let mut active = 1;
        handoff.publish(Publication::Recovery, 2).unwrap();
        consumer.activate(Publication::Recovery, &mut active);
        assert_eq!(active, 2);
        assert!(!handoff.idle());
        handoff.reclaim();
        handoff.publish(Publication::Graph, 3).unwrap();
        consumer.activate(Publication::Recovery, &mut active);
        assert_eq!(active, 2);
        assert!(handoff.reserve_format().is_none());
        assert!(handoff.publish(Publication::Recovery, 4).is_err());
        consumer.activate(Publication::Graph, &mut active);
        assert_eq!(active, 3);
        handoff.reclaim();
        assert!(handoff.idle());
    }

    #[test]
    fn format_token_excludes_both_lanes_and_drop_releases_reservation() {
        let handoff = ResourceHandoff::default();
        let token = handoff.reserve_format().unwrap();
        assert!(handoff.reserve_format().is_none());
        assert!(handoff.publish(Publication::Graph, 1).is_err());
        assert!(handoff.publish(Publication::Recovery, 2).is_err());
        handoff.reclaim();
        assert!(!handoff.idle());
        drop(token);
        assert!(handoff.idle());
        handoff.publish(Publication::Recovery, 3).unwrap();
    }

    #[test]
    fn committed_format_keeps_graph_reservation_until_activation_and_reclamation() {
        let handoff = ResourceHandoff::default();
        let mut consumer = handoff.try_consumer().unwrap();
        let token = handoff.reserve_format().unwrap();
        handoff.publish_reserved(&token, 7);
        drop(token);
        assert!(!handoff.idle());
        assert!(handoff.reserve_format().is_none());
        assert!(handoff.publish(Publication::Recovery, 8).is_err());
        let mut active = 1;
        consumer.activate(Publication::Recovery, &mut active);
        assert_eq!(active, 1);
        consumer.activate(Publication::Graph, &mut active);
        assert_eq!(active, 7);
        assert!(!handoff.idle());
        handoff.reclaim();
        assert!(handoff.idle());
    }

    #[test]
    fn partial_consumer_claim_rolls_back_without_releasing_the_other_lane() {
        let handoff = ResourceHandoff::<u64>::default();
        let recovery = handoff.recovery.try_consumer().unwrap();
        assert!(handoff.try_consumer().is_none());
        assert!(handoff.graph.try_consumer().is_some());
        assert!(handoff.recovery.try_consumer().is_none());
        drop(recovery);
        let consumer = handoff.try_consumer().unwrap();
        assert!(handoff.try_consumer().is_none());
        drop(consumer);
        assert!(handoff.try_consumer().is_some());
    }
}
