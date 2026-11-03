//! Protocol admission and response projection for the engine controller.

use crate::{
    AppStatus, EngineGraphController, ErrorCode, ExternalProcessorCatalog, GraphApplyRequest,
    GraphApplyResponse, GraphDocument, GraphRuntimeStatus, MeterSnapshot, PROTOCOL_VERSION,
    ProcessorEditorStatus, ProcessorParameterUpdateResponse, ProcessorRetryRequest,
    ProcessorScanRequest, ProtocolError, SessionReconfigureRequest, node_control,
    processor_editor_error, processor_parameter_error,
};

impl EngineGraphController {
    pub fn telemetry_state(&self) -> Option<mixrelay_control_protocol::ControlStateSnapshot> {
        self.observe_control_state()
    }
    /// # Errors
    /// Returns a revision or committed-control observation failure.
    pub fn controls_snapshot(
        &self,
    ) -> Result<mixrelay_control_protocol::EngineControlSnapshot, ProtocolError> {
        self.snapshot_controls()
    }
    /// # Errors
    /// Rejects an unknown graph or unavailable committed controls.
    pub fn node_controls(
        &self,
        graph_id: &str,
    ) -> Result<mixrelay_control_protocol::GraphNodeControls, ProtocolError> {
        self.read_node_controls(graph_id)
    }

    /// # Errors
    /// Rejects stale revisions, unavailable controls or failed graph preparation.
    pub fn update_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlResponse, ProtocolError> {
        self.change_node_control(graph_id, node_id, request)
    }

    /// # Errors
    /// Rejects stale revisions or controls that cannot be previewed.
    pub fn preview_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: &mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlPreview, ProtocolError> {
        self.preview_node_switch(graph_id, node_id, request)
    }
    /// # Errors
    /// Reports invalid workspace data or storage observation failures.
    pub fn preview_workspace_file(
        &self,
        request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<mixrelay_control_protocol::WorkspacePreview, ProtocolError> {
        self.preview_workspace(request)
    }
    /// # Errors
    /// Reports rejected workspace admission or unpublished storage failures.
    pub fn workspace_file(
        &self,
        request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.workspace_operation(request).map_err(|mut error| {
            // Every fallible preparation precedes publication. Once accepted, storage
            // warnings travel in GraphCatalogResponse rather than rejecting the request.
            if let Some(parameters) = error.parameters.as_object_mut() {
                parameters.insert("workspaceOutcome".into(), serde_json::json!("not_applied"));
            }
            error
        })
    }
    /// # Errors
    /// Rejects a graph or recorder node that is not present.
    pub fn recorder_status(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        self.snapshot_recorder_status(graph_id, node_id)
    }

    /// # Errors
    /// Reports stale take identity, admission or recorder-local failures.
    pub fn control_recorder(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::RecorderControlRequest,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        let _mutation = self.try_node_control()?;
        self.validate_control_revision(request.expected_revision)?;
        let id = self.recorder_session_id(graph_id, node_id)?;
        if let Some(expected) = request.expected_recording {
            let current = self.recorders.status(&id);
            if expected.state != current.state || expected.path != current.path {
                return Err(node_control::node_error(
                    ErrorCode::GraphRevisionStale,
                    graph_id,
                    node_id,
                ));
            }
        }
        let request = request.request;
        if matches!(
            request.action,
            mixrelay_audio_io::recording::RecorderAction::Stop
        ) && self.recorders.status(&id).state
            == mixrelay_audio_io::recording::RecorderState::Stopped
        {
            return Ok(self.recorders.status(&id));
        }
        if matches!(
            request.action,
            mixrelay_audio_io::recording::RecorderAction::Record
                | mixrelay_audio_io::recording::RecorderAction::Pause
        ) {
            let graph = self.catalog_document_locked(graph_id)?;
            if !self.power.enabled()
                || !graph
                    .nodes
                    .iter()
                    .any(|node| node.id.as_str() == node_id && node.control.enabled)
            {
                return Err(node_control::node_error(
                    ErrorCode::NodeDisabled,
                    graph_id,
                    node_id,
                ));
            }
        }
        self.recorders.control(id.as_str(), request)
    }
    /// # Errors
    /// Reports authored capture bindings that cannot be resolved.
    pub fn application_capture_manifest(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        self.current_capture_manifest()
    }

    /// # Errors
    /// Reports invalid graphs or unsupported capture bindings.
    pub fn preview_application_capture(
        &self,
        request: GraphApplyRequest,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        self.preview_capture_manifest(request)
    }
    /// # Errors
    /// Reports unsupported endpoint settings or a failed native rate acknowledgement.
    pub fn reconfigure_virtual_endpoint(
        &self,
        request: &mixrelay_control_protocol::VirtualEndpointFormatRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.change_endpoint_format(request)
    }

    /// # Errors
    /// Rejects an unknown graph or unavailable catalog.
    pub fn graph_layout(
        &self,
        id: &str,
    ) -> Result<mixrelay_control_protocol::GraphLayoutResponse, ProtocolError> {
        self.read_graph_layout(id)
    }

    /// # Errors
    /// Reports stale layout revisions or rejected layout data.
    pub fn update_graph_layout(
        &self,
        id: &str,
        request: mixrelay_control_protocol::GraphLayoutRequest,
    ) -> Result<mixrelay_control_protocol::GraphLayoutResponse, ProtocolError> {
        self.write_graph_layout(id, request)
    }
    /// # Errors
    /// Reports an unavailable catalog.
    pub fn graph_catalog(
        &self,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.read_catalog()
    }

    /// # Errors
    /// Rejects an absent graph or unavailable catalog.
    pub fn graph_catalog_document(&self, id: &str) -> Result<GraphDocument, ProtocolError> {
        self.read_catalog_document(id)
    }

    /// # Errors
    /// Reports revision conflicts, validation or publication failures.
    pub fn mutate_graph_catalog(
        &self,
        request: mixrelay_control_protocol::GraphCatalogRequest,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.mutate_catalog(request)
    }
    /// # Errors
    /// Reports unsupported format changes or failed native negotiation.
    pub fn reconfigure_session(
        &self,
        request: SessionReconfigureRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.change_session(request)
    }
    pub fn status(&self) -> AppStatus {
        let format = self.context.read().format;
        let mut status = self.runtime_status.status(format);
        status.engine_power = Some(self.engine_power_status());
        status.processor_timings = self.processor_timings();
        status.processor_latencies = self.processor_latencies();
        status.failed_processors = self.failed_processor_status();
        status.recorder_error = self.recorders.cleanup_error();
        if let Some(session) = &mut status.audio_session {
            session.virtual_endpoint_settings = self.endpoint_settings();
            if status
                .engine_power
                .is_some_and(|power| !power.enabled && !power.transitioning)
            {
                session.processing_state = mixrelay_audio_io::process::ProcessingState::Stopped;
            }
        }
        status
    }

    pub fn graph_status(&self) -> GraphRuntimeStatus {
        let revisions = self.exchange.revisions();
        GraphRuntimeStatus {
            pending_reason: self
                .runtime_status
                .awaiting_microphone_permission()
                .then_some(
                    mixrelay_control_protocol::GraphPendingReason::AwaitingMicrophonePermission,
                ),
            protocol_version: PROTOCOL_VERSION,
            active_revision: revisions.active,
            submitted_revision: revisions.submitted,
        }
    }

    pub fn graph(&self) -> GraphDocument {
        self.graph.read().clone()
    }

    pub fn meters(&self) -> MeterSnapshot {
        self.meter_snapshot_at(self.clock.now())
    }

    pub fn incarnation(&self) -> String {
        self.incarnation.clone()
    }

    pub fn processors(&self) -> ExternalProcessorCatalog {
        self.processor_catalog.protocol_catalog()
    }

    /// # Errors
    /// Reports an invalid scan path before catalog discovery starts.
    pub fn rescan_processors(
        &self,
        request: ProcessorScanRequest,
    ) -> Result<ExternalProcessorCatalog, ProtocolError> {
        let paths = request
            .paths
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        self.processor_catalog.rescan(&paths)?;
        Ok(self.processor_catalog.protocol_catalog())
    }

    /// # Errors
    /// Reports stale revisions, invalid values or unavailable processor controls.
    pub fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        request: mixrelay_control_protocol::ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        self.apply_processor_parameter_update(
            node_id,
            parameter_id,
            request.value,
            |provider, runtime_id| {
                self.validate_control_revision(request.expected_revision)?;
                if !self
                    .processor_catalog
                    .parameter_is_writable(provider.type_id.as_str(), parameter_id)
                {
                    return Err(processor_parameter_error(
                        node_id,
                        parameter_id,
                        "processor parameter is unavailable or read-only",
                    ));
                }
                self.processor_controls
                    .set_parameter(runtime_id, parameter_id, request.value)
                    .map_err(|reason| {
                        processor_parameter_error(node_id, parameter_id, &reason.to_string())
                    })
            },
        )
    }

    /// # Errors
    /// Reports an absent or unavailable processor editor.
    pub fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        let _mutation = self.mutation.lock();
        self.processor_editor_status_locked(node_id)
    }

    /// # Errors
    /// Rejects stale worker identities, cooldown or failed replacement preparation.
    pub fn retry_failed_processor(
        &self,
        request: &ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.retry_failed_processor_locked(request)
    }

    /// # Errors
    /// Reports an absent editor or rejected native editor command.
    pub fn set_processor_editor(
        &self,
        node_id: &str,
        request: mixrelay_control_protocol::ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        let _mutation = self.mutation.lock();
        self.validate_control_revision(request.expected_revision)?;
        let open = request.open;
        let runtime_id = self.external_processor_node(node_id)?;
        if !open {
            self.processor_controls
                .cancel_editor_gesture(&runtime_id)
                .map_err(|reason| processor_editor_error(node_id, &reason.to_string()))?;
            // Preserve vendor edits while the instance is still available. A failed
            // durable checkpoint must remain retryable, not silently close the editor.
            self.checkpoint_processor_state_locked(Some(node_id))?;
        }
        let mut status = self
            .processor_controls
            .set_editor_open(&runtime_id, open)
            .map_err(|reason| processor_editor_error(node_id, &reason.to_string()))?;
        node_id.clone_into(&mut status.node_id);
        Ok(status)
    }

    /// # Errors
    /// Reports an unavailable editor or failed frame capture.
    pub fn processor_editor_frame(
        &self,
        node_id: &str,
    ) -> Result<mixrelay_control_protocol::ProcessorEditorFrame, ProtocolError> {
        let runtime_id = {
            let _mutation = self.mutation.lock();
            self.external_processor_node(node_id)?
        };
        self.processor_controls
            .editor_frame(&runtime_id)
            .map_err(|reason| processor_editor_error(node_id, &reason.to_string()))
    }

    /// # Errors
    /// Reports stale editor input or rejected native delivery.
    pub fn processor_editor_input(
        &self,
        node_id: &str,
        input: &mixrelay_control_protocol::ProcessorEditorInput,
    ) -> Result<(), ProtocolError> {
        let runtime_id = {
            let _mutation = self.mutation.lock();
            self.external_processor_node(node_id)?
        };
        self.processor_controls
            .editor_input(&runtime_id, input)
            .map_err(|reason| processor_editor_error(node_id, &reason.to_string()))
    }

    /// # Errors
    /// Reports stale revisions or a failed pause or resume transaction.
    pub fn set_engine_power(
        &self,
        request: mixrelay_control_protocol::EnginePowerRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.change_engine_power(request.enabled, request.expected_revision)
    }

    /// # Errors
    /// Reports revision conflicts, rejected graph admission or failed preparation.
    pub fn apply_graph_request(
        &self,
        request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        // Selection changes take the same mutation lock, so this check and the apply are atomic.
        if let Some(expected) = &request.expected_graph_id {
            let selected = self.view_graph_id();
            if *expected != selected {
                return Err(ProtocolError {
                    code: ErrorCode::GraphRevisionStale,
                    parameters: serde_json::json!({
                        "expectedGraphId": expected,
                        "selectedGraphId": selected,
                    }),
                });
            }
        }
        let reuse = self.processor_controls.reuse_transaction();
        let mut document = request.document;
        self.preserve_live_processor_state(&mut document, &request.processor_parameter_changes)?;
        let catalog = self.catalog_candidate(&document);
        let result = self.apply_graph_locked(document, catalog)?;
        reuse.commit();
        Ok(result)
    }
}
