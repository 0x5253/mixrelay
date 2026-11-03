//! Instance-scoped accounting for prepared, active and retiring stream allocations.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Counts only acquisitions created in this native resource scope.
/// Clones share a scope; independent registries never affect each other's retirement.
#[derive(Debug, Clone, Default)]
pub struct StreamOwners(Arc<Owners>);

#[derive(Debug, Default)]
struct Owners {
    active: AtomicUsize,
    #[cfg(any(test, feature = "test-observation"))]
    next_observation: std::sync::atomic::AtomicU64,
}

impl StreamOwners {
    /// Includes prepared, active and retiring streams through final native teardown.
    #[must_use]
    pub fn active_owners(&self) -> usize {
        self.0.active.load(Ordering::Acquire)
    }

    pub(super) fn acquire(&self) -> StreamOwner {
        // Publish the live native allocation before related control owners inspect the scope.
        self.0.active.fetch_add(1, Ordering::AcqRel);
        StreamOwner(self.clone())
    }

    #[cfg(any(test, feature = "test-observation"))]
    pub(super) fn observation_identity(&self) -> u64 {
        // Diagnostic identities need only distinguish allocations within this resource scope.
        self.0.next_observation.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub(super) struct StreamOwner(StreamOwners);
impl Drop for StreamOwner {
    fn drop(&mut self) {
        // Final native drain precedes token drop, so zero acknowledges complete reclamation.
        self.0.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests;
