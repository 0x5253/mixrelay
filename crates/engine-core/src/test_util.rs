//! Portable port composition for adapter-only fixture controllers.
use crate::{EngineGraphControllerConfig, RenderFormat};

/// Builds a controller configuration without native devices, workers or file access.
/// Native callback fixtures replace the clock explicitly with their native clock owner.
#[must_use]
pub fn configuration(format: RenderFormat) -> EngineGraphControllerConfig {
    crate::test_support::configuration(format)
}

/// Fixture-only access to supplied ownership and transaction boundaries.
impl crate::EngineGraphController {
    #[must_use]
    pub fn test_factory(&self) -> std::sync::Arc<dyn crate::ProcessorFactory> {
        self.factory.clone()
    }
    pub fn test_set_factory(&mut self, factory: std::sync::Arc<dyn crate::ProcessorFactory>) {
        self.factory = factory;
    }
    /// # Errors
    /// Reports the same post-staging publication rejection as the controller transaction.
    pub fn test_publish_staged_graph(
        &self,
        document: crate::GraphDocument,
    ) -> Result<crate::GraphApplyResponse, crate::ProtocolError> {
        let catalog = self.catalog_candidate(&document);
        self.publish_graph_locked(document, catalog)
    }
    /// # Errors
    /// Reports checkpoint admission failures; capture is forbidden by this fixture.
    /// # Panics
    /// Panics if the providerless checkpoint attempts native capture.
    pub fn test_checkpoint_without_providers(&self) -> Result<(), crate::ProtocolError> {
        let _mutation = self.mutation.lock();
        self.checkpoint_processor_state_with(None, |_, _| panic!("no native provider exists"))
    }
    #[must_use]
    pub fn test_context(&self) -> crate::CompileContext {
        *self.context.read()
    }
    #[must_use]
    pub fn test_graph(&self) -> &parking_lot::RwLock<crate::GraphDocument> {
        &self.graph
    }
    /// # Errors
    /// Reports the same preparation rejection as normal graph publication.
    pub fn test_prepare_processor(
        &self,
        document: &crate::GraphDocument,
        context: crate::CompileContext,
    ) -> Result<Box<mixrelay_render::BlockProcessor>, mixrelay_control_protocol::ProtocolError>
    {
        self.prepare_and_validate(document, context)
            .map(|prepared| prepared.processor)
    }
    /// # Errors
    /// Rejects an absent graph or recorder node.
    pub fn test_recorder_session_id(
        &self,
        graph: &str,
        node: &str,
    ) -> Result<String, mixrelay_control_protocol::ProtocolError> {
        self.recorder_session_id(graph, node)
    }
    pub fn test_retry_pause_save(&self) {
        self.retry_pause_save();
    }
    pub fn test_reclaim_recorders(&self) {
        self.reclaim_recorders();
    }
}

impl crate::EngineGraphController {
    /// # Errors
    /// Reports the normal composed-execution admission failure.
    pub fn test_composed_document(
        &self,
        document: &crate::GraphDocument,
        context: crate::CompileContext,
    ) -> Result<crate::GraphDocument, mixrelay_control_protocol::ProtocolError> {
        self.compose_execution(document, None, context)
            .map(|composed| composed.document)
    }
    pub fn test_set_catalog_path(&self, path: std::path::PathBuf) {
        self.catalog.test_set_path(path);
    }
}
