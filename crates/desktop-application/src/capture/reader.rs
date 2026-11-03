//! RAII claim for a shared telemetry owner with exactly one SPSC PCM reader.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub trait ReaderOwner {
    fn reader_claim(&self) -> &AtomicBool;
}

/// Deliberately not Clone. Moving this token transfers the only consumer;
/// dropping it releases the claim while its Arc still keeps the owner alive.
#[derive(Debug)]
pub struct ExclusiveReader<S: ReaderOwner> {
    owner: Arc<S>,
}

impl<S: ReaderOwner> ExclusiveReader<S> {
    pub fn acquire(owner: Arc<S>) -> Option<Self> {
        owner
            .reader_claim()
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Self { owner })
    }

    #[must_use]
    pub fn owner(&self) -> &S {
        &self.owner
    }
}

impl<S: ReaderOwner> Drop for ExclusiveReader<S> {
    fn drop(&mut self) {
        self.owner.reader_claim().store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
