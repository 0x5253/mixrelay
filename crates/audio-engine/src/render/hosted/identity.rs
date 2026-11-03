//! Retained allocation identity for an exact hosted processor ownership transfer.

use std::{cmp::Ordering, fmt, sync::Arc};

/// A key retained by one processor instance and its prepared reuse requests.
/// Create and clone keys during control-thread preparation. Rendering borrows them.
/// Retained keys keep their allocation alive through plan retirement.
#[derive(Clone)]
pub struct HostedProcessorIdentity(Arc<()>);

impl HostedProcessorIdentity {
    /// Creates a fresh identity without consulting process-global state.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl Default for HostedProcessorIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HostedProcessorIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HostedProcessorIdentity")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for HostedProcessorIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for HostedProcessorIdentity {}
impl PartialOrd for HostedProcessorIdentity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HostedProcessorIdentity {
    fn cmp(&self, other: &Self) -> Ordering {
        Arc::as_ptr(&self.0).cmp(&Arc::as_ptr(&other.0))
    }
}
