//! Stops a crash loop. Every service run leaves a marker that only a complete, clean exit
//! removes, so the next start knows whether the previous run ended unexpectedly. After
//! `LIMIT` unexpected ends within `WINDOW`, the engine restores its graph paused and says
//! why, instead of failing again on the same graph until the user notices.
//!
//! The service arms the guard once it alone owns the engine (before any fallible start-up
//! work) and finishes it only after every owner has shut down successfully.
use std::{
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mixrelay_engine_core::crash_policy::{RunMarker, next_marker};

pub(crate) const RUN_MARKER: &str = "engine-run.json";
/// Serializes reading, replacing and removing the marker across engine processes.
pub(crate) const RUN_LOCK: &str = "engine-run.lock";
/// This run's marker; dropping it without `finish` leaves the run counted as unexpected.
#[derive(Debug)]
pub struct CrashGuard {
    marker: PathBuf,
    run: String,
}

impl CrashGuard {
    /// Records this service run in the engine's configuration directory. Returns the guard
    /// and whether the previous runs ended unexpectedly often enough that this one should
    /// start paused. Storage failures never pause the engine; they only lose the history.
    #[must_use]
    pub fn arm(directory: io::Result<PathBuf>) -> (Option<Self>, bool) {
        match directory {
            Ok(directory) => {
                let (guard, tripped) = Self::start(&directory.join("graphs.json"));
                (Some(guard), tripped)
            }
            Err(error) => {
                tracing::warn!(%error, "no configuration directory for the engine run marker");
                (None, false)
            }
        }
    }

    pub(crate) fn start(catalog: &Path) -> (Self, bool) {
        Self::start_at(catalog, SystemTime::now())
    }

    fn start_at(catalog: &Path, now: SystemTime) -> (Self, bool) {
        let marker = catalog.with_file_name(RUN_MARKER);
        let _lock = lock(&marker);
        let previous = match std::fs::read(&marker) {
            Ok(bytes) => Some(serde_json::from_slice(&bytes).unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(%error, "could not read the previous engine run");
                None
            }
        };
        let seconds = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let (next, tripped) = next_marker(previous, crate::identity::new_incarnation(), seconds);
        let run = next.run().to_owned();
        if let Err(error) = crate::persistence::stage_json(&marker, &next)
            .and_then(crate::persistence::StagedWrite::commit)
        {
            tracing::warn!(%error, "could not record the engine run");
        }
        if tripped {
            tracing::error!("the engine ended unexpectedly repeatedly; starting paused");
        }
        (Self { marker, run }, tripped)
    }

    /// A clean exit: the next start does not count this run. A marker another run wrote
    /// since (or none at all) is left alone.
    pub fn finish(self) {
        let _lock = lock(&self.marker);
        let ours = std::fs::read(&self.marker)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RunMarker>(&bytes).ok())
            .is_some_and(|marker| marker.run() == self.run);
        if !ours {
            return;
        }
        if let Err(error) = std::fs::remove_file(&self.marker) {
            tracing::warn!(%error, "could not record the clean engine exit");
        }
    }
}

/// Holds the marker lock until dropped; without it (storage failure) the marker is used
/// unlocked, which only risks the history, never audio.
fn lock(marker: &Path) -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(marker.with_file_name(RUN_LOCK))
        .and_then(|file| file.lock().map(|()| file));
    file.inspect_err(|error| tracing::warn!(%error, "could not lock the engine run marker"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn three_unexpected_ends_within_the_window_trip_once() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-guard").unwrap();
        let catalog = directory.join("graphs.json");
        // A clean run leaves nothing to count.
        let (guard, tripped) = CrashGuard::start_at(&catalog, at(1_000));
        assert!(!tripped);
        guard.finish();
        assert!(!catalog.with_file_name(RUN_MARKER).exists());
        // A finishing run leaves a successor's marker, and a missing one, alone.
        let (first, _) = CrashGuard::start_at(&catalog, at(1_001));
        let (second, _) = CrashGuard::start_at(&catalog, at(1_002));
        first.finish();
        assert!(catalog.with_file_name(RUN_MARKER).exists());
        second.finish();
        assert!(!catalog.with_file_name(RUN_MARKER).exists());
        CrashGuard {
            marker: catalog.with_file_name(RUN_MARKER),
            run: String::new(),
        }
        .finish();
        // Three runs end without finishing; the fourth start pauses and resets the count.
        for (second, expected) in [
            (1_010, false),
            (1_020, false),
            (1_030, false),
            (1_040, true),
        ] {
            let (_guard, tripped) = CrashGuard::start_at(&catalog, at(second));
            assert_eq!(tripped, expected, "start at {second}");
        }
        let (_guard, tripped) = CrashGuard::start_at(&catalog, at(1_050));
        assert!(!tripped, "the count restarts after the guard paused");
    }

    #[test]
    fn old_unexpected_ends_and_unreadable_markers_do_not_count() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-guard-bad").unwrap();
        let catalog = directory.join("graphs.json");
        std::fs::write(catalog.with_file_name(RUN_MARKER), b"not json").unwrap();
        // An unreadable marker still means an unexpected end, at the epoch: too old to count.
        let (_guard, tripped) = CrashGuard::start_at(&catalog, at(10_000));
        assert!(!tripped);
    }

    #[test]
    fn arming_uses_the_configuration_directory_when_there_is_one() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-guard-arm").unwrap();
        let (guard, tripped) = CrashGuard::arm(Ok(directory.path().to_path_buf()));
        assert!(!tripped);
        assert!(directory.join(RUN_MARKER).is_file());
        guard.unwrap().finish();
        assert!(!directory.join(RUN_MARKER).exists());
        let (guard, tripped) = CrashGuard::arm(Err(io::Error::other("no home")));
        assert!(guard.is_none() && !tripped);
    }

    #[test]
    fn a_marker_that_cannot_be_removed_is_left_for_the_next_start() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-guard-ro").unwrap();
        let catalog = directory.join("graphs.json");
        let (guard, _) = CrashGuard::start_at(&catalog, at(1));
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        guard.finish();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(catalog.with_file_name(RUN_MARKER).exists());
    }

    #[test]
    fn storage_failures_never_pause_the_engine() {
        let directory = mixrelay_test_support::FixtureDir::new("engine-crash-guard-io").unwrap();
        // The marker's and lock's places are directories: neither can be used.
        let catalog = directory.join("graphs.json");
        std::fs::create_dir(catalog.with_file_name(RUN_MARKER)).unwrap();
        std::fs::create_dir(catalog.with_file_name(RUN_LOCK)).unwrap();
        let (guard, tripped) = CrashGuard::start_at(&catalog, at(1));
        assert!(!tripped);
        guard.finish();
    }
}
