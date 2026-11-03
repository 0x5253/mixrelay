use super::{
    Catalog, EngineGraphController, GraphDocument, NodeConfig, ProtocolError, WorkspaceAction,
    WorkspaceRequest, file_path, invalid, load_fingerprinted, unavailable,
};
use mixrelay_control_protocol::WorkspacePreview;

impl EngineGraphController {
    pub(crate) fn preview_workspace(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspacePreview, ProtocolError> {
        let _mutation = self.mutation.lock();
        let current = self
            .catalog_candidate(&self.graph.read())
            .ok_or_else(|| unavailable("graph catalog is unavailable"))?;
        self.require_active_revision(request.expected_revision)?;
        if request.expected_catalog_revision != current.revision {
            return Err(invalid(
                "workspace selection changed before capture preparation",
            ));
        }
        self.require_stopped_recorders(&current)?;
        let (mut candidate, digest) = match request.operation {
            WorkspaceAction::New => (
                Catalog::imported(GraphDocument::new(0, vec![], vec![])),
                None,
            ),
            WorkspaceAction::Open { path } => {
                let (catalog, digest, _) =
                    load_fingerprinted(self.documents.as_ref(), file_path(&path)?)?;
                (catalog, Some(digest))
            }
            WorkspaceAction::Save { .. } => {
                return Err(invalid("saving does not need capture preparation"));
            }
        };
        let revision = candidate
            .next_audio_revision(request.expected_revision)
            .ok_or_else(|| invalid("audio revision overflow"))?;
        // Native clocks have not changed yet. This detached validation copy observes
        // current rates only; the digest binds the final request to original file settings.
        for (_, document) in candidate.documents_mut() {
            document.revision = revision;
            for node in &mut document.nodes {
                if let NodeConfig::VirtualEndpoint(endpoint) = &mut node.config
                    && let Ok(rate) = self.endpoint_backend.sample_rate(&endpoint.id)
                {
                    endpoint.spec.default_sample_rate = rate;
                    if !endpoint.spec.supported_sample_rates.contains(&rate) {
                        endpoint.spec.supported_sample_rates.push(rate);
                    }
                }
            }
        }
        let composed = self.compose_execution(
            candidate.active_document(),
            Some(&candidate),
            *self.context.read(),
        )?;
        Ok(WorkspacePreview {
            manifest: crate::execution_control::composed_capture_manifest(&composed)?,
            file_digest: digest,
        })
    }
}
