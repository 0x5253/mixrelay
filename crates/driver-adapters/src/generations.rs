//! Calendar-time candidates for service-issued latency generations.

use mixrelay_driver_core::LatencyGenerations;

/// Uses epoch nanoseconds as generation candidates across service restarts.
#[derive(Debug, Clone, Copy)]
pub struct SystemLatencyGenerations;

impl LatencyGenerations for SystemLatencyGenerations {
    fn candidate(&self) -> u64 {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        u64::try_from(epoch).unwrap_or(u64::MAX)
    }
}
