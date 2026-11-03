//! Crash-loop pause decisions over supplied run history and epoch seconds.
use serde::{Deserialize, Serialize};
use std::time::Duration;
const LIMIT: usize = 3;
const WINDOW: Duration = Duration::from_mins(2);

#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMarker {
    /// Identifies the run that wrote the marker, so a finishing run never removes a
    /// successor's marker.
    #[serde(default)]
    run: String,
    started_at: u64,
    /// Start times (seconds since the epoch) of recent runs that never exited cleanly.
    #[serde(default)]
    unexpected_ends: Vec<u64>,
}

/// The marker this run writes, and whether the runs before it form a crash loop. A run
/// that tripped the guard starts the count again, so resuming needs `LIMIT` new failures
/// before the guard pauses again.
#[must_use]
pub fn next_marker(previous: Option<RunMarker>, run: String, now: u64) -> (RunMarker, bool) {
    let mut ends = Vec::new();
    if let Some(previous) = previous {
        // The previous run left its marker behind: it did not exit cleanly.
        ends = previous.unexpected_ends;
        ends.push(previous.started_at);
    }
    ends.retain(|&end| now.saturating_sub(end) <= WINDOW.as_secs());
    let tripped = ends.len() >= LIMIT;
    if tripped {
        ends.clear();
    }
    (
        RunMarker {
            run,
            started_at: now,
            unexpected_ends: ends,
        },
        tripped,
    )
}

impl RunMarker {
    #[must_use]
    pub fn run(&self) -> &str {
        &self.run
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_failures_do_not_pause_a_new_run() {
        assert_eq!(
            next_marker(
                Some(RunMarker {
                    run: "old".to_owned(),
                    started_at: 500,
                    unexpected_ends: vec![100, 200],
                }),
                "new".to_owned(),
                600,
            ),
            (
                RunMarker {
                    run: "new".to_owned(),
                    started_at: 600,
                    unexpected_ends: vec![500],
                },
                false,
            )
        );
    }
}
