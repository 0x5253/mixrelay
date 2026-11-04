//! Startup recall retains saved documents independently of transient native availability.

use mixrelay_time::Instant;
use parking_lot::Mutex;
use std::time::Duration;

use mixrelay_audio_io::ApplicationSelector;
use mixrelay_control_protocol::{GraphApplyResponse, ProtocolError};

use super::{EngineGraphController, GraphDocument, NodeConfig, invalid};

/// Retry deadline and last reported native restore failure; never persisted as catalog data.
#[derive(Debug)]
struct PendingRestore {
    retry_after: Instant,
    error: ProtocolError,
}

#[derive(Default)]
pub(crate) struct CatalogRecall {
    pending: Mutex<Option<PendingRestore>>,
}

impl CatalogRecall {
    pub(crate) fn defer(&self, error: ProtocolError, now: Instant) {
        let mut pending = self.pending.lock();
        if pending.as_ref().is_none_or(|previous| {
            previous.error.code != error.code || previous.error.parameters != error.parameters
        }) {
            tracing::warn!(
                ?error,
                "saved graph native restore deferred; catalog retained"
            );
        }
        *pending = Some(PendingRestore {
            retry_after: now.saturating_add(Duration::from_secs(1)),
            error,
        });
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.pending.lock().is_some()
    }

    pub(crate) fn ready(&self, now: Instant) -> bool {
        self.pending
            .lock()
            .as_ref()
            .is_some_and(|pending| now >= pending.retry_after)
    }

    pub(crate) fn complete(&self) {
        *self.pending.lock() = None;
    }
}

impl EngineGraphController {
    /// The caller owns the mutation lock, or exclusive startup builder ownership.
    pub(super) fn restore_catalog_locked(&self) -> Result<GraphApplyResponse, ProtocolError> {
        let mut document = self.graph.read().clone();
        document.revision = self
            .catalog
            .read(|value| value.next_audio_revision(self.exchange.revisions().submitted))?
            .ok_or_else(|| invalid("audio revision overflow"))?;
        let candidate = self.observed_catalog_candidate(&mut document)?;
        self.apply_graph_locked(document, candidate)
    }

    /// Records the failure without invalidating the durable catalog, logging only changes.
    pub(crate) fn defer_catalog_restore(&self, error: ProtocolError) {
        self.catalog_restore.defer(error, self.clock.now());
    }

    /// Retries startup recall only on the existing non-real-time maintenance worker.
    /// A concurrent control mutation defers this attempt; status reads never trigger it.
    pub(crate) fn retry_catalog_restore(&self) {
        self.retry_catalog_restore_at(self.clock.now());
    }

    fn retry_catalog_restore_at(&self, now: Instant) {
        let Some(_mutation) = self.mutation.try_lock() else {
            return;
        };
        if !self.catalog_restore.ready(now) {
            return;
        }
        match self.restore_catalog_locked() {
            Ok(response) => tracing::info!(
                revision = response.revision,
                "saved graph native restore recovered"
            ),
            Err(error) => self.defer_catalog_restore(error),
        }
    }
}

pub(super) fn require_pid_reselection(document: &mut GraphDocument) {
    for node in &mut document.nodes {
        if let NodeConfig::ApplicationCapture(request) = &mut node.config
            && matches!(request.selector, ApplicationSelector::ProcessId { .. })
        {
            request.requires_reselection = true;
        }
    }
}

#[cfg(test)]
#[path = "graph_restore_tests.rs"]
mod tests;
