//! Service identity and time sources supplied by Driver composition.

use std::{fmt, num::NonZeroU32};

use mixrelay_time::MonotonicClock;

use crate::DriverError;

/// Supplies a generation candidate distinct from earlier service lifetimes.
/// The authority advances its own counter when a candidate is older.
pub trait LatencyGenerations: Send {
    /// The candidate for the next accepted engine claim.
    fn candidate(&self) -> u64;
}

/// The process that owns the Driver control service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverProcessId(NonZeroU32);

impl DriverProcessId {
    /// Validates an operating-system process identifier.
    /// # Errors
    /// Rejects zero and values outside the signed native PID range.
    pub fn new(value: u32) -> Result<Self, DriverError> {
        let value = NonZeroU32::new(value)
            .filter(|value| i32::try_from(value.get()).is_ok())
            .ok_or_else(|| DriverError::Invalid("invalid Driver process identity".into()))?;
        Ok(Self(value))
    }

    /// The identifier carried by Driver snapshots.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// The runtime inputs shared by inventory and latency decisions.
pub struct DriverRuntime {
    pub(crate) owner: DriverProcessId,
    pub(crate) clock: Box<dyn MonotonicClock>,
    pub(crate) generations: Box<dyn LatencyGenerations>,
}

impl DriverRuntime {
    /// Connects an owner to explicitly selected clock and identity sources.
    #[must_use]
    pub fn new(
        owner: DriverProcessId,
        clock: Box<dyn MonotonicClock>,
        generations: Box<dyn LatencyGenerations>,
    ) -> Self {
        Self {
            owner,
            clock,
            generations,
        }
    }
}

impl fmt::Debug for DriverRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DriverRuntime")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
