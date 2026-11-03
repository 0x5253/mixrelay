//! Authoritative state captured from a prepared processor instance.

use std::time::Duration;

use crate::ProcessorStateError;

pub(super) trait StateCapture: Send + Sync {
    /// Captures native state within the caller's remaining per-instance budget.
    fn snapshot(&self, timeout: Duration) -> Result<Vec<u8>, ProcessorStateError>;
}

#[cfg(test)]
pub(crate) mod memory;
