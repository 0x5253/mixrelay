//! Delegates the server adapter contract to the engine application service.
use crate::{
    AppStatus, ControlBackend, EngineGraphController, ExternalProcessorCatalog, GraphApplyRequest,
    GraphApplyResponse, GraphDocument, GraphRuntimeStatus, MeterSnapshot, ProcessorEditorStatus,
    ProcessorParameterUpdateResponse, ProcessorRetryRequest, ProcessorScanRequest, ProtocolError,
    SessionReconfigureRequest,
};
impl ControlBackend for EngineGraphController {
    fn telemetry_state(&self) -> Option<mixrelay_control_protocol::ControlStateSnapshot> {
        self.core.telemetry_state()
    }
    fn controls_snapshot(
        &self,
    ) -> Result<mixrelay_control_protocol::EngineControlSnapshot, ProtocolError> {
        self.core.controls_snapshot()
    }
    fn node_controls(
        &self,
        graph_id: &str,
    ) -> Result<mixrelay_control_protocol::GraphNodeControls, ProtocolError> {
        self.core.node_controls(graph_id)
    }
    fn update_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlResponse, ProtocolError> {
        self.core.update_node_control(graph_id, node_id, &request)
    }
    fn preview_node_control(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::NodeControlRequest,
    ) -> Result<mixrelay_control_protocol::NodeControlPreview, ProtocolError> {
        self.core.preview_node_control(graph_id, node_id, &request)
    }
    fn preview_workspace_file(
        &self,
        request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<mixrelay_control_protocol::WorkspacePreview, ProtocolError> {
        self.core.preview_workspace_file(request)
    }
    fn workspace_file(
        &self,
        request: mixrelay_control_protocol::WorkspaceRequest,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.core.workspace_file(request)
    }
    fn recorder_status(
        &self,
        graph_id: &str,
        node_id: &str,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        self.core.recorder_status(graph_id, node_id)
    }
    fn control_recorder(
        &self,
        graph_id: &str,
        node_id: &str,
        request: mixrelay_control_protocol::RecorderControlRequest,
    ) -> Result<mixrelay_audio_io::recording::RecorderStatus, ProtocolError> {
        self.core.control_recorder(graph_id, node_id, request)
    }
    fn application_capture_manifest(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        self.core.application_capture_manifest()
    }
    fn preview_application_capture(
        &self,
        request: GraphApplyRequest,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        self.core.preview_application_capture(request)
    }
    fn reconfigure_virtual_endpoint(
        &self,
        request: mixrelay_control_protocol::VirtualEndpointFormatRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.core.reconfigure_virtual_endpoint(&request)
    }
    fn graph_layout(
        &self,
        id: &str,
    ) -> Result<mixrelay_control_protocol::GraphLayoutResponse, ProtocolError> {
        self.core.graph_layout(id)
    }
    fn update_graph_layout(
        &self,
        id: &str,
        request: mixrelay_control_protocol::GraphLayoutRequest,
    ) -> Result<mixrelay_control_protocol::GraphLayoutResponse, ProtocolError> {
        self.core.update_graph_layout(id, request)
    }
    fn graph_catalog(
        &self,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.core.graph_catalog()
    }
    fn graph_catalog_document(&self, id: &str) -> Result<GraphDocument, ProtocolError> {
        self.core.graph_catalog_document(id)
    }
    fn mutate_graph_catalog(
        &self,
        request: mixrelay_control_protocol::GraphCatalogRequest,
    ) -> Result<mixrelay_control_protocol::GraphCatalogResponse, ProtocolError> {
        self.core.mutate_graph_catalog(request)
    }
    fn reconfigure_session(
        &self,
        request: SessionReconfigureRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.core.reconfigure_session(request)
    }
    fn status(&self) -> AppStatus {
        self.core.status()
    }
    fn graph_status(&self) -> GraphRuntimeStatus {
        self.core.graph_status()
    }
    fn graph(&self) -> GraphDocument {
        self.core.graph()
    }
    fn meters(&self) -> MeterSnapshot {
        self.core.meters()
    }
    fn incarnation(&self) -> String {
        self.core.incarnation()
    }
    fn processors(&self) -> ExternalProcessorCatalog {
        self.core.processors()
    }
    fn rescan_processors(
        &self,
        request: ProcessorScanRequest,
    ) -> Result<ExternalProcessorCatalog, ProtocolError> {
        self.core.rescan_processors(request)
    }
    fn set_processor_parameter(
        &self,
        node_id: &str,
        parameter_id: u32,
        request: mixrelay_control_protocol::ProcessorParameterUpdate,
    ) -> Result<ProcessorParameterUpdateResponse, ProtocolError> {
        self.core
            .set_processor_parameter(node_id, parameter_id, request)
    }
    fn processor_editor_status(
        &self,
        node_id: &str,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        self.core.processor_editor_status(node_id)
    }
    fn retry_failed_processor(
        &self,
        request: ProcessorRetryRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.core.retry_failed_processor(&request)
    }
    fn set_processor_editor(
        &self,
        node_id: &str,
        request: mixrelay_control_protocol::ProcessorEditorRequest,
    ) -> Result<ProcessorEditorStatus, ProtocolError> {
        self.core.set_processor_editor(node_id, request)
    }
    fn processor_editor_frame(
        &self,
        node_id: &str,
    ) -> Result<mixrelay_control_protocol::ProcessorEditorFrame, ProtocolError> {
        self.core.processor_editor_frame(node_id)
    }
    fn processor_editor_input(
        &self,
        node_id: &str,
        input: mixrelay_control_protocol::ProcessorEditorInput,
    ) -> Result<(), ProtocolError> {
        self.core.processor_editor_input(node_id, &input)
    }
    fn set_engine_power(
        &self,
        request: mixrelay_control_protocol::EnginePowerRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.core.set_engine_power(request)
    }
    fn preview_engine_start(
        &self,
    ) -> Result<mixrelay_control_protocol::ApplicationCaptureManifest, ProtocolError> {
        self.core.preview_engine_start()
    }
    fn apply_graph_request(
        &self,
        request: GraphApplyRequest,
    ) -> Result<GraphApplyResponse, ProtocolError> {
        self.core.apply_graph_request(request)
    }
}
