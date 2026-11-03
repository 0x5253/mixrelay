use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use mixrelay_control_protocol::EnginePauseReason;
use parking_lot::Mutex;

/// A pause outlives the service: a restarted or rebooted engine stays paused until
/// Resume. Running is the default, so Resume removes the marker.
pub(super) const SAVED_PAUSE: &str = "engine-paused.json";
/// Saved with a pause the engine chose after repeated unexpected ends.
const REPEATED_FAILURES: &str = "repeatedFailures";

pub(super) struct ExecutionChoice {
    execution_enabled: AtomicBool,
    paused_after_failures: AtomicBool,
    saved_pause: Option<PathBuf>,
    save_warning: Mutex<Option<String>>,
    documents: std::sync::Arc<dyn crate::document_files::DocumentFiles>,
}

impl ExecutionChoice {
    pub(super) fn new(documents: std::sync::Arc<dyn crate::document_files::DocumentFiles>) -> Self {
        Self {
            execution_enabled: AtomicBool::new(true),
            paused_after_failures: AtomicBool::new(false),
            saved_pause: None,
            save_warning: Mutex::new(None),
            documents,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.execution_enabled.load(Ordering::Acquire)
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        self.execution_enabled.store(enabled, Ordering::Release);
    }

    pub(super) fn pause_after_failures(&self) {
        self.set_enabled(false);
        self.paused_after_failures.store(true, Ordering::Release);
    }

    pub(super) fn clear_failure_reason(&self) {
        self.paused_after_failures.store(false, Ordering::Release);
    }

    pub(super) fn pause_reason(&self, enabled: bool) -> Option<EnginePauseReason> {
        (!enabled && self.paused_after_failures.load(Ordering::Acquire))
            .then_some(EnginePauseReason::RepeatedFailures)
    }

    pub(super) fn unsaved(&self) -> bool {
        self.save_warning.lock().is_some()
    }

    pub(super) fn warning(&self) -> Option<String> {
        self.save_warning.lock().clone()
    }

    pub(super) fn restore(&mut self, catalog: &Path) {
        let marker = catalog.with_file_name(SAVED_PAUSE);
        if self.documents.exists(&marker) {
            tracing::info!("engine stays paused from the previous session");
            self.execution_enabled = AtomicBool::new(false);
            // The reason stays until Resume, across restarts; an unreadable one is dropped,
            // never the pause itself.
            let saved: serde_json::Value = {
                let mut bytes = Vec::new();
                self.documents
                    .open(&marker)
                    .and_then(|mut input| input.read_to_end(&mut bytes))
                    .ok()
                    .and_then(|_| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default()
            };
            if saved["reason"] == REPEATED_FAILURES {
                self.paused_after_failures = AtomicBool::new(true);
            }
        }
        self.saved_pause = Some(marker);
        if self.paused_after_failures.load(Ordering::Acquire) {
            // Like a user pause: the graph is restored without audio until Resume.
            self.persist(false);
        }
    }

    fn save(&self, enabled: bool) -> io::Result<()> {
        let Some(marker) = &self.saved_pause else {
            return Ok(());
        };
        if !enabled {
            let mut saved = serde_json::json!({ "enabled": false });
            if self.paused_after_failures.load(Ordering::Acquire) {
                saved["reason"] = REPEATED_FAILURES.into();
            }
            return crate::document_files::stage_json(self.documents.as_ref(), marker, &saved)
                .and_then(crate::document_files::StagedWrite::commit);
        }
        match self.documents.remove(marker) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            removed => removed,
        }
    }

    /// A failed save is reported with the other save warnings and retried by control
    /// maintenance until storage accepts it.
    pub(super) fn persist(&self, enabled: bool) {
        let mut warning = self.save_warning.lock();
        match self.save(enabled) {
            Ok(()) if warning.take().is_some() => {
                tracing::info!(enabled, "engine pause choice saved after retry");
            }
            Ok(()) => {}
            Err(error) => {
                if warning.is_none() {
                    tracing::error!(%error, enabled, "engine pause choice was not saved");
                }
                *warning = Some(format!("engine pause: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_files::{DocumentFiles, StagedWrite, WriteDocument};
    use std::{io::Read, sync::Arc};
    struct UnreadableMarker;
    impl DocumentFiles for UnreadableMarker {
        fn exists(&self, _: &Path) -> bool {
            true
        }
        fn open(&self, _: &Path) -> io::Result<Box<dyn Read + Send>> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
        fn remove(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn stage(&self, _: &Path, _: WriteDocument<'_>) -> io::Result<StagedWrite> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }
    #[test]
    fn an_unreadable_existing_pause_marker_keeps_processing_paused() {
        let mut choice = ExecutionChoice::new(Arc::new(UnreadableMarker));
        choice.restore(Path::new("/configuration/graphs.json"));
        assert!(!choice.enabled());
        assert_eq!(choice.pause_reason(false), None);
    }
}
