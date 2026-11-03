//! Execution is independent of authored documents and control-service lifetime.
//! Empty publication uses the same acknowledged resource handoff as graph edits.
use mixrelay_control_protocol::{EnginePowerStatus, GraphApplyResponse, GraphApplyState};

use crate::{EngineGraphController, ProtocolError, session_error};

impl EngineGraphController {
    /// Starts paused because the service's crash guard found repeated unexpected ends.
    /// Apply before the durable catalog, which saves the pause and its reason.
    #[must_use]
    pub fn after_repeated_failures(self, tripped: bool) -> Self {
        if tripped {
            self.power.pause_after_failures();
        }
        self
    }

    pub(super) fn retry_pause_save(&self) {
        if !self.power.unsaved() {
            return;
        }
        // Serialized with power changes, so a retry cannot save a superseded choice.
        let _mutation = self.mutation.lock();
        self.power.persist(self.power.enabled());
    }

    /// # Errors
    /// Reports authored capture sources that cannot be resolved before resume.
    pub fn preview_engine_start(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        let _mutation = self.mutation.lock();
        if self.power.enabled() || self.engine_power_status().transitioning {
            return Err(session_error("engine must be off before start preflight"));
        }
        let mut document = self.graph.read().clone();
        document.revision = self
            .exchange
            .revisions()
            .submitted
            .checked_add(1)
            .ok_or_else(|| session_error("engine revision exhausted"))?;
        let catalog = self.catalog_candidate(&document);
        let composed =
            self.compose_enabled_execution(&document, catalog.as_ref(), *self.context.read())?;
        crate::execution_control::composed_capture_manifest(&composed)
    }
    pub(super) fn engine_power_status(&self) -> EnginePowerStatus {
        let enabled = self.power.enabled();
        let revisions = self.exchange.revisions();
        EnginePowerStatus {
            enabled,
            transitioning: revisions.active != revisions.submitted
                || (!enabled && !self.graph_resources.execution_released()),
            unsaved: self.power.unsaved(),
            pause_reason: self.power.pause_reason(enabled),
        }
    }

    pub(super) fn change_engine_power(
        &self,
        enabled: bool,
        expected_revision: Option<u64>,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        let previous = self.power.enabled();
        let revisions = self.exchange.revisions();
        if expected_revision.is_some_and(|expected| expected != revisions.submitted) {
            return Err(session_error("engine power preflight is stale"));
        }
        if enabled == previous {
            if enabled {
                self.recorders.resume()?;
            }
            return Ok(GraphApplyResponse {
                protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
                revision: revisions.submitted,
                state: GraphApplyState::Pending,
                persistence_error: self.graph_persistence_warning(),
            });
        }
        if revisions.active != revisions.submitted
            || (!previous && !self.graph_resources.execution_released())
        {
            return Err(session_error("engine execution is still transitioning"));
        }
        // A broken plug-in checkpoint must not prevent the user releasing audio.
        let checkpoint_error = if previous {
            self.checkpoint_processor_state_locked(None).err()
        } else {
            None
        };
        let mut document = self.graph.read().clone();
        document.revision = revisions
            .submitted
            .checked_add(1)
            .ok_or_else(|| session_error("engine revision exhausted"))?;
        let catalog = self.catalog_candidate(&document);
        if !enabled {
            self.recorders.suspend()?;
        }
        self.power.set_enabled(enabled);
        let context = *self.context.read();
        // The choice is saved once published, before recorder resume can fail.
        let result = self
            .publish_graph_context(document, catalog, context, enabled)
            .map(|mut response| {
                self.power.persist(enabled);
                response.persistence_error = self.graph_persistence_warning();
                response
            });
        if result.is_err() {
            self.power.set_enabled(previous);
            if !enabled {
                self.recorders.resume()?;
            }
        } else if enabled {
            self.power.clear_failure_reason();
            self.recorders.resume()?;
        }
        if let Some(error) = checkpoint_error {
            tracing::error!(?error, "engine stop retained last saved plug-in state");
            self.catalog
                .record_warning(Some(format!("engine stop checkpoint: {error:?}")));
        }
        tracing::info!(
            enabled,
            accepted = result.is_ok(),
            "engine execution switch"
        );
        result
    }
}
