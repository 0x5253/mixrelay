//! Deterministic runtime inputs for Driver policy tests.

use mixrelay_time::{Instant, ManualClock};
use std::time::Duration;

use crate::{DriverProcessId, DriverRuntime, LatencyGenerations};

#[derive(Debug)]
pub struct FixedGenerations(pub u64);

impl LatencyGenerations for FixedGenerations {
    fn candidate(&self) -> u64 {
        self.0
    }
}

#[must_use]
pub fn runtime() -> DriverRuntime {
    let Ok(owner) = DriverProcessId::new(42) else {
        unreachable!("fixture process ID is valid");
    };
    DriverRuntime::new(
        owner,
        Box::new(ManualClock::new(Instant::from_origin(Duration::ZERO))),
        Box::new(FixedGenerations(100)),
    )
}
