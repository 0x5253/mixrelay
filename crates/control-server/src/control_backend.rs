//! Transport-independent control operations and their unavailable defaults.
use mixrelay_audio_graph::GraphDocument;
use mixrelay_control_protocol::{
    AppStatus, ControlStateSnapshot, ErrorCode, ExternalProcessorCatalog, GraphApplyRequest,
    GraphApplyResponse, GraphCatalogRequest, GraphCatalogResponse, GraphLayoutRequest,
    GraphLayoutResponse, GraphRuntimeStatus, MeterSnapshot, PROTOCOL_VERSION, ProcessorEditorFrame,
    ProcessorEditorInput, ProcessorEditorRequest, ProcessorEditorStatus, ProcessorParameterUpdate,
    ProcessorParameterUpdateResponse, ProcessorRetryRequest, ProcessorScanRequest, ProtocolError,
    SessionReconfigureRequest,
};

pub trait ControlBackend: Send + Sync {
    /// Observes state without preparing native resources. A busy writer may defer this
    /// observation: the publisher retains the last committed snapshot until it succeeds.
    fn telemetry_state(&self) -> Option<mixrelay_control_protocol::ControlStateSnapshot> {
        Some(snapshot(self))
    }

    /// Reads all targets, controls and recorder states from one engine observation.
    /// # Errors
    /// Reports unavailable control state; ordinary concurrent mutations do not fail reads.
    fn controls_snapshot(
        &self,
    ) -> Result<mixrelay_control_protocol::EngineControlSnapshot, ProtocolError> {
        Err(node_control_unavailable())
    }

    /// Discovers authored node controls in one graph without changing selection.
    /// # Errors
    /// Reports an unknown graph or unavailable backend.
    fn node_controls(
        &self,
        _graph_id: &str,
    ) -> Result<mixrelay_control_protocol::GraphNodeControls, ProtocolError> {
        Err(node_control_unavailable())
    }

    /// Preflights a revision-guarded node control patch without publishing it.
    /// # Errors
    /// Rejects missing targets, stale revisions and unsupported controls.
    fn preview_node_control(
        &self,
        _graph_id: &str,
        _node_id: &str,
        _request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlPreview, ProtocolError> {
        Err(node_control_unavailable())
    }

    /// Publishes explicit node state through the shared control owner.
    /// # Errors
    /// Rejects missing targets, stale revisions and native preparation failures.
    fn update_node_control(
        &self,
        _graph_id: &str,
        _node_id: &str,
        _request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlResponse, ProtocolError> {
        Err(node_control_unavailable())
    }
    /// Preflights the next start without acquiring audio or changing execution.
    /// # Errors
    /// Rejects non-stopped engines and invalid graph/native selections.
    fn preview_engine_start(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"engine_power"}),
        })
    }
    /// Publishes or retires execution without changing authored graph contents.
    /// # Errors
    /// Reports unsupported control, publication contention or native preparation errors.
    fn set_engine_power(
        &self,
        _request: mixrelay_control_protocol::EnginePowerRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"engine_power"}),
        })
    }
    /// Prepares capture metadata without publishing the requested workspace.
    /// # Errors
    /// Rejects invalid files, stale revisions and unavailable composition resources.
    fn preview_workspace_file(
        &self,
        _request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<mixrelay_control_protocol::WorkspacePreview, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"workspace_preview"}),
        })
    }
    /// Saves or replaces a complete authored workspace on the control thread.
    /// # Errors
    /// Rejects stale revisions, invalid files, active recordings, and preparation failures.
    fn workspace_file(
        &self,
        _request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"workspace"}),
        })
    }
    /// Reads one graph-owned recording without changing the selected graph.
    /// # Errors
    /// Reports an unavailable recorder or unknown graph/node identity.
    fn recorder_status(
        &self,
        _graph_id: &str,
        _node_id: &str,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        Err(recorder_unavailable())
    }

    /// Starts, resumes, pauses, or finalizes a recording on the control thread.
    /// # Errors
    /// Reports format, path, state, or file errors without interrupting audio output.
    fn control_recorder(
        &self,
        _graph_id: &str,
        _node_id: &str,
        _request: mixrelay_control_protocol::RecorderControlRequest,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        Err(recorder_unavailable())
    }
    /// Returns the canonical capture owners for the complete running catalog.
    /// # Errors
    /// Reports unavailable discovery or invalid capture ownership without fabricating sources.
    fn application_capture_manifest(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"application_capture_manifest"}),
        })
    }

    /// Validates the full candidate catalog and describes capture owners without publishing.
    /// # Errors
    /// Rejects stale/busy revisions, invalid topology and unsupported source policies.
    fn preview_application_capture(
        &self,
        _request: GraphApplyRequest,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"application_capture_preview"}),
        })
    }
    /// Changes the own virtual device's format without targeting other devices.
    /// # Errors
    /// Rejects stale revisions, unsupported formats or native failures.
    fn reconfigure_virtual_endpoint(
        &self,
        _request: mixrelay_control_protocol::VirtualEndpointFormatRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::AudioSessionUnsupported,
            parameters: serde_json::json!({"resource":"virtual_endpoint_format"}),
        })
    }
    /// Reads presentation metadata independently of the active DSP graph.
    /// # Errors
    /// Returns unavailable or unknown-identity errors without synthesizing layout entries.
    fn graph_layout(&self, _id: &str) -> Result<GraphLayoutResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"graph_layout"}),
        })
    }
    /// Atomically saves one graph's presentation without rebuilding its audio resources.
    /// # Errors
    /// Rejects unknown identities, invalid values, stale layout revisions and storage failures.
    fn update_graph_layout(
        &self,
        _id: &str,
        _request: GraphLayoutRequest,
    ) -> Result<GraphLayoutResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"graph_layout"}),
        })
    }
    /// Returns an inactive document for native preflight without changing the active graph.
    /// # Errors
    /// Rejects missing identities or unavailable persistence.
    fn graph_catalog_document(&self, _id: &str) -> Result<GraphDocument, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }
    /// Returns the durable graph collection, never an invented UI-only list.
    /// # Errors
    /// Returns an unavailable error if catalog persistence could not be loaded.
    fn graph_catalog(&self) -> Result<GraphCatalogResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"reason": "graph catalog unavailable"}),
        })
    }
    /// Serializes catalog edits with graph publication.
    /// Accepted execution changes return a response even if durable replacement fails;
    /// report that failure through `persistence_error`, never through `Err`.
    /// # Errors
    /// Rejects stale writers and invalid or unavailable graphs without changing the
    /// catalog, selected document, or execution. HTTP clients rely on `Err` as not applied.
    fn mutate_graph_catalog(
        &self,
        _request: GraphCatalogRequest,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        self.graph_catalog().and_then(|_| {
            Err(ProtocolError {
                code: ErrorCode::ControlUnavailable,
                parameters: serde_json::json!({"reason": "graph catalog mutations unavailable"}),
            })
        })
    }
    /// Reconfigures one engine-wide audio session, independently from endpoint channel maps.
    /// # Errors
    /// Returns a protocol error without pretending an unsupported adapter changed formats.
    fn reconfigure_session(
        &self,
        _request: SessionReconfigureRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::AudioSessionUnsupported,
            parameters: serde_json::json!({}),
        })
    }
    fn status(&self) -> AppStatus;
    fn graph(&self) -> GraphDocument;
    fn graph_status(&self) -> GraphRuntimeStatus;
    fn meters(&self) -> MeterSnapshot;
    /// Identifies this engine process; a restarted engine has a new one.
    fn incarnation(&self) -> String {
        String::new()
    }
    fn processors(&self) -> ExternalProcessorCatalog;

    /// Rebuilds the external processor catalog from standard and explicit directories.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when a path is invalid or discovery cannot be started.
    fn rescan_processors(
        &self,
        request: ProcessorScanRequest,
    ) -> Result<ExternalProcessorCatalog, ProtocolError>;

    /// Updates one parameter on an active hosted processor.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError`] when the node or parameter is unavailable, or when the
    /// processor worker cannot accept the value.
    fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        update: ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError>;

    /// Returns the current native editor state for one active processor node.
    ///
    /// # Errors
    ///
    /// Returns a stable protocol failure when the node or worker is unavailable.
    fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError>;

    /// Requests an idempotent native editor transition for one active processor node.
    ///
    /// # Errors
    ///
    /// Returns a stable protocol failure when the node or worker is unavailable.
    fn set_processor_editor(
        &self,
        node_id: &str,
        request: ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError>;

    /// Reprepares one exact active failed provider from its committed saved state.
    /// # Errors
    /// Rejects stale graph/catalog revisions, disabled or removed nodes, and stale worker IDs.
    fn retry_failed_processor(
        &self,
        _request: ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ProcessorUnavailable,
            parameters: serde_json::json!({"reason":"processor retry is unsupported"}),
        })
    }

    /// Captures a bounded native image without blocking the async HTTP executor.
    /// # Errors
    /// Returns a protocol failure for an unavailable or busy editor.
    fn processor_editor_frame(&self, node_id: &str) -> Result<ProcessorEditorFrame, ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    /// Publishes ordered input for one identified native editor session.
    /// # Errors
    /// Rejects unavailable sessions and invalid or saturated input.
    fn processor_editor_input(
        &self,
        node_id: &str,
        _input: ProcessorEditorInput,
    ) -> Result<(), ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    /// Validates and submits a graph document and its explicit parameter intent to the runtime.
    ///
    /// # Errors
    ///
    /// Returns a stable [`ProtocolError`] when validation, binding, revision,
    /// authorization, or processor submission fails.
    fn apply_graph_request(
        &self,
        request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError>;
}

#[derive(Debug, Default)]
/// The backend of a server without an engine: reads are empty and every change is refused.
pub(crate) struct UnavailableBackend;

impl ControlBackend for UnavailableBackend {
    fn status(&self) -> AppStatus {
        AppStatus::default()
    }

    fn graph_status(&self) -> GraphRuntimeStatus {
        GraphRuntimeStatus {
            pending_reason: None,
            protocol_version: PROTOCOL_VERSION,
            active_revision: 0,
            submitted_revision: 0,
        }
    }

    fn graph(&self) -> GraphDocument {
        GraphDocument::new(0, vec![], vec![])
    }

    fn meters(&self) -> MeterSnapshot {
        MeterSnapshot {
            protocol_version: PROTOCOL_VERSION,
            revision: 0,
            nodes: Vec::new(),
        }
    }

    fn processors(&self) -> ExternalProcessorCatalog {
        ExternalProcessorCatalog {
            protocol_version: PROTOCOL_VERSION,
            processors: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn rescan_processors(
        &self,
        _request: ProcessorScanRequest,
    ) -> Result<ExternalProcessorCatalog, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }

    fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        _update: ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ProcessorUnavailable,
            parameters: serde_json::json!({
                "nodeId": node_id,
                "parameterId": parameter_id,
            }),
        })
    }

    fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    fn set_processor_editor(
        &self,
        node_id: &str,
        _request: ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        Err(processor_unavailable(node_id))
    }

    fn retry_failed_processor(
        &self,
        _request: ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({"resource":"processor_retry"}),
        })
    }

    fn apply_graph_request(
        &self,
        _request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        Err(ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }
}

fn processor_unavailable(node_id: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ProcessorUnavailable,
        parameters: serde_json::json!({ "nodeId": node_id }),
    }
}

fn node_control_unavailable() -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ControlUnavailable,
        parameters: serde_json::json!({"resource": "node_control"}),
    }
}

fn recorder_unavailable() -> ProtocolError {
    ProtocolError {
        code: ErrorCode::RecorderFailed,
        parameters: serde_json::json!({"reason": "recorder unavailable"}),
    }
}

pub(crate) fn snapshot(backend: &(impl ControlBackend + ?Sized)) -> ControlStateSnapshot {
    let editors = backend
        .graph()
        .nodes
        .iter()
        .filter_map(|node| backend.processor_editor_status(node.id.as_str()).ok())
        .collect();
    ControlStateSnapshot {
        generation: 0,
        status: backend.status(),
        graph: backend.graph_status(),
        controls: backend.controls_snapshot().ok(),
        editors,
    }
}
