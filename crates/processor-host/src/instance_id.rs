//! Nonzero processor identities issued by one clone-shared control registry.

use std::{
    fmt,
    num::NonZeroU64,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InstanceId(NonZeroU64);

impl InstanceId {
    pub(crate) fn new(value: u64) -> Option<Self> {
        NonZeroU64::new(value).map(Self)
    }
    pub(crate) fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("processor instance identifiers are exhausted")]
pub(super) struct InstancesExhausted;

#[derive(Debug)]
pub(super) struct InstanceIds {
    next: AtomicU64,
}

impl InstanceIds {
    pub(super) fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    pub(super) fn issue(&self) -> Result<InstanceId, InstancesExhausted> {
        // Identity allocation orders no shared data; the CAS only reserves a unique number.
        // Zero marks terminal exhaustion after issuing u64::MAX and can never be returned.
        let issued = self
            .next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (next != 0).then(|| next.checked_add(1).unwrap_or(0))
            })
            .map_err(|_| InstancesExhausted)?;
        InstanceId::new(issued).ok_or(InstancesExhausted)
    }
}

#[cfg(test)]
mod tests;
