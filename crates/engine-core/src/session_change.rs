//! Serializes format adoption, native rollback and the matching DSP publication.

use crate::{
    CompileContext, EngineGraphController, ErrorCode, GraphApplyResponse, GraphApplyState,
    HostAudioBackendError, PROTOCOL_VERSION, PlaybackBindings, PreparedGraphFormat, ProtocolError,
    RenderFormat, SessionReconfigureRequest, document_files as persistence, execution_control,
    graph_catalog,
};

impl EngineGraphController {
    /// Reconciles an external OS/DAW rate change on a non-real-time control thread.
    /// No native setter is invoked in this path. A pending graph or
    /// another mutation defers reconciliation until the monitor's next observation.
    /// # Errors
    /// Reports unsupported formats/transports and native or preset preparation failures.
    pub fn reconcile_observed_device_rate(
        &self,
    ) -> Result<Option<GraphApplyResponse>, ProtocolError> {
        let Some(_mutation) = self.mutation.try_lock() else {
            return Ok(None);
        };
        let current = self.context.read().format;
        let Some(observed) = self.runtime_status.external_format(current) else {
            return Ok(None);
        };
        let revisions = self.exchange.revisions();
        if revisions.active != revisions.submitted {
            return Ok(None);
        }
        if observed == current {
            return self.reconcile_endpoint_clock_locked();
        }
        self.change_session_locked(
            SessionReconfigureRequest {
                expected_revision: revisions.submitted,
                format: mixrelay_audio_io::session::SessionFormat {
                    sample_rate: observed.sample_rate,
                    block_frames: observed.block_frames,
                },
            },
            SessionChangeOrigin::Observed,
            SessionPauseOwner::SessionChange,
        )
        .map(Some)
    }

    pub(super) fn change_session(
        &self,
        request: SessionReconfigureRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        self.change_session_locked(
            request,
            SessionChangeOrigin::User,
            SessionPauseOwner::SessionChange,
        )
    }

    pub(super) fn reconcile_endpoint_clock_locked(
        &self,
    ) -> Result<Option<GraphApplyResponse>, ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        let mut document = self.graph.read().clone();
        let mut catalog = self.catalog_candidate(&document);
        if !self.observe_endpoint_catalog(&mut document, &mut catalog)? {
            return Ok(None);
        }
        document.revision = document
            .revision
            .checked_add(1)
            .ok_or_else(|| session_error("revision overflow"))?;
        self.preserve_live_processor_state(&mut document, &[])?;
        if let Some(candidate) = &mut catalog {
            candidate.update_document(document.clone());
        }
        let result = self.apply_graph_locked(document, catalog)?;
        reuse.commit();
        Ok(Some(result))
    }

    pub(super) fn change_session_locked(
        &self,
        request: SessionReconfigureRequest,
        origin: SessionChangeOrigin,
        pause_owner: SessionPauseOwner,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        let current = *self.context.read();
        let revisions = self.require_active_revision(request.expected_revision)?;
        self.validate_session_request(request, current.format, origin)?;
        if request.format.sample_rate == current.format.sample_rate
            && request.format.block_frames == current.format.block_frames
        {
            return self.accept_unchanged_session(
                current.format,
                revisions.active,
                origin,
                pause_owner,
            );
        }
        let mut document = self.graph.read().clone();
        document.revision = revisions
            .submitted
            .checked_add(1)
            .ok_or_else(|| session_error("revision overflow"))?;
        let next = CompileContext {
            format: RenderFormat {
                sample_rate: request.format.sample_rate,
                block_frames: request.format.block_frames,
            },
            ..current
        };
        self.preserve_live_processor_state(&mut document, &[])?;
        let mut catalog = self.observed_catalog_candidate(&mut document)?;
        if let Some(candidate) = &mut catalog {
            self.preserve_catalog_provider_state(candidate)?;
        }
        let staged_catalog = self.stage_session_catalog(catalog.as_ref(), origin)?;
        let execution_control::PreparedExecution {
            processor,
            document: runtime_document,
            view,
            playback_intents,
            ..
        } = self.prepare_execution(&document, catalog.as_ref(), next)?;
        let mut resources = self
            .graph_resources
            .prepare_format_change(
                &runtime_document,
                PlaybackBindings::from_plan(&playback_intents, &view.controls),
                current.format,
                next.format,
            )
            .map_err(|error| session_error(&error.to_string()))?;
        let bindings = processor.plan().meter_bindings().to_vec();
        let revision = document.revision;
        let result = self.exchange.submit_configured(processor, || {
            self.configure_session_native(current.format, next.format, origin, resources.as_mut())?;
            resources.commit();
            self.finish_catalog_write(staged_catalog);
            self.publish_execution_metadata(revision, bindings, view);
            *self.context.write() = next;
            *self.graph.write() = document;
            self.replace_catalog_memory(catalog);
            Ok::<(), HostAudioBackendError>(())
        });
        if let Err(error) = result {
            self.resume_session(pause_owner);
            return Err(session_error(&error.to_string()));
        }
        self.refresh_control_snapshot();
        // Resume only after the prepared plan is published; the worker activates it before
        // processing another block. Active format reporting belongs to that worker.
        self.resume_session(pause_owner);
        reuse.commit();
        Ok(GraphApplyResponse {
            protocol_version: PROTOCOL_VERSION,
            revision,
            state: GraphApplyState::Pending,
            persistence_error: self.graph_persistence_warning(),
        })
    }

    /// Device/session mutations share the same settled revision precondition.
    /// The caller holds the mutation lock throughout validation and publication.
    pub(super) fn require_active_revision(
        &self,
        expected: u64,
    ) -> Result<mixrelay_render::ProcessorRevisions, ProtocolError> {
        let revisions = self.exchange.revisions();
        if expected != revisions.submitted || revisions.active != revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({
                    "activeRevision": revisions.active,
                    "submittedRevision": revisions.submitted,
                }),
            });
        }
        Ok(revisions)
    }

    pub(super) fn configure_session_native(
        &self,
        current: RenderFormat,
        next: RenderFormat,
        origin: SessionChangeOrigin,
        resources: &mut dyn PreparedGraphFormat,
    ) -> Result<(), HostAudioBackendError> {
        if matches!(origin, SessionChangeOrigin::Observed) {
            return self.runtime_status.adopt_external_format(next);
        }
        self.runtime_status.reconfigure(current, next)?;
        if let Err(error) = resources.configure_native() {
            self.runtime_status
                .reconfigure(next, current)
                .map_err(|rollback| HostAudioBackendError::BackendFailure {
                    message: format!("{error}; session rollback failed: {rollback}"),
                })?;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn accept_unchanged_session(
        &self,
        format: RenderFormat,
        revision: u64,
        origin: SessionChangeOrigin,
        pause_owner: SessionPauseOwner,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        // Saving/recovering the same format must not recreate plug-ins or invalidate history.
        if origin == SessionChangeOrigin::User {
            let result = self.runtime_status.reconfigure(format, format);
            self.resume_session(pause_owner);
            result.map_err(|error| session_error(&error.to_string()))?;
        }
        Ok(GraphApplyResponse {
            protocol_version: PROTOCOL_VERSION,
            revision,
            state: GraphApplyState::Active,
            persistence_error: self.graph_persistence_warning(),
        })
    }

    pub(super) fn resume_session(&self, pause_owner: SessionPauseOwner) {
        // An enclosing endpoint negotiation owns its suspension through native
        // rollback acknowledgement. An inner adoption cannot release that pause.
        if pause_owner == SessionPauseOwner::SessionChange {
            self.runtime_status.resume();
        }
    }

    pub(super) fn stage_session_catalog(
        &self,
        catalog: Option<&graph_catalog::Catalog>,
        origin: SessionChangeOrigin,
    ) -> Result<Option<persistence::StagedWrite>, ProtocolError> {
        match self.stage_catalog(catalog) {
            Ok(staged) => Ok(staged),
            Err(error) if origin == SessionChangeOrigin::Observed => {
                self.catalog.record_warning(Some(format!(
                    "external format catalog save failed: {error:?}"
                )));
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn validate_session_request(
        &self,
        request: SessionReconfigureRequest,
        current: RenderFormat,
        origin: SessionChangeOrigin,
    ) -> Result<(), ProtocolError> {
        let supported = self
            .runtime_status
            .status(current)
            .audio_session
            .is_some_and(|session| {
                (origin == SessionChangeOrigin::Observed
                    || session.capabilities.can_reconfigure_live)
                    && session
                        .capabilities
                        .supported_sample_rates
                        .contains(&request.format.sample_rate)
                    && session
                        .capabilities
                        .supported_block_frames
                        .contains(&request.format.block_frames)
            });
        if !supported {
            return Err(ProtocolError {
                code: ErrorCode::AudioSessionUnsupported,
                parameters: serde_json::json!({"reason": "requested session format is unavailable"}),
            });
        }
        // Native transport format changes are separately opt-in through a prepared
        // resource transaction; a broad OS capability does not bypass that boundary.
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionChangeOrigin {
    User,
    Observed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SessionPauseOwner {
    SessionChange,
    EndpointChange,
}

pub(super) fn session_error(reason: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::AudioSessionReconfigureFailed,
        parameters: serde_json::json!({"reason": reason}),
    }
}
