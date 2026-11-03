//! Builds one execution snapshot from authored catalog documents. Native resource and
//! meter identities come from that same snapshot; editor selection never owns capture.

use std::collections::BTreeMap;

use mixrelay_audio_graph::{
    CompileContext, GraphDocument, NodeConfig, NodeId, VirtualEndpointSpec,
};
use mixrelay_audio_io::ApplicationCaptureRequest;
use mixrelay_control_protocol::{
    ApplicationCaptureManifest, ApplicationCaptureSource, ErrorCode, GraphApplyRequest,
    PROTOCOL_VERSION, ProtocolError,
};
use mixrelay_render::BlockProcessor;

use crate::catalog_execution::{
    ApplicationIdentityResolver, CatalogExecutionContext, compose_catalog,
};
use crate::graph_catalog::Catalog;
use crate::graph_composition::{ComposedGraph, CompositionError, NodeOrigin, SharedSource};
use crate::{
    EngineGraphController, GraphResourceBackend, prepare_error, validate_endpoint_bindings,
    virtual_endpoint_specs,
};

pub(super) struct PreparedExecution {
    pub processor: Box<BlockProcessor>,
    pub document: GraphDocument,
    pub endpoints: Vec<VirtualEndpointSpec>,
    pub view: ExecutionView,
    pub playback_intents: Vec<crate::graph_composition::ApplicationPlaybackIntent>,
}

#[derive(Clone)]
pub(super) struct ExecutionView {
    pub origins: BTreeMap<NodeId, NodeOrigin>,
    pub captures: ApplicationCaptureManifest,
    pub playback_intents: Vec<crate::graph_composition::ApplicationPlaybackIntent>,
    pub controls: std::sync::Arc<mixrelay_render::RuntimeControlBank>,
    pub native_controls: BTreeMap<NodeId, Vec<NodeId>>,
    pub prepared_native: std::collections::BTreeSet<NodeId>,
}

struct NativeIdentities<'a>(&'a dyn GraphResourceBackend);

impl ApplicationIdentityResolver for NativeIdentities<'_> {
    fn resolve(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<SharedSource, CompositionError> {
        self.0
            .resolve_application_capture(request)
            .map_err(|error| CompositionError::Invalid(error.to_string()))
    }
}

impl EngineGraphController {
    pub(crate) fn view_graph_id(&self) -> String {
        self.catalog.inspect(|value| {
            value.and_then(|value| value.as_ref().ok()).map_or_else(
                || "session".into(),
                |catalog| catalog.selected_id().to_owned(),
            )
        })
    }

    pub(super) fn compose_execution(
        &self,
        document: &GraphDocument,
        catalog: Option<&Catalog>,
        context: CompileContext,
    ) -> Result<ComposedGraph, ProtocolError> {
        if !self.power.enabled() {
            return compose_catalog(
                &[],
                CatalogExecutionContext {
                    revision: document.revision,
                    compile: context,
                    compiler: &self.factory.compiler(),
                    devices: &[],
                    applications: &NativeIdentities(self.graph_resources.as_ref()),
                },
            )
            .map_err(composition_error);
        }
        self.compose_enabled_execution(document, catalog, context)
    }

    pub(super) fn compose_enabled_execution(
        &self,
        document: &GraphDocument,
        catalog: Option<&Catalog>,
        context: CompileContext,
    ) -> Result<ComposedGraph, ProtocolError> {
        let mut documents: Vec<_> = catalog.map_or_else(
            || vec![(self.view_graph_id(), document.clone())],
            |catalog| {
                catalog
                    .documents()
                    .map(|(id, graph)| (id.to_owned(), graph.clone()))
                    .collect()
            },
        );
        self.resolve_execution_devices(&mut documents)?;
        for (_, graph) in &documents {
            self.validate_session_bindings(graph, context.format)?;
        }
        let selectors: Vec<_> = documents
            .iter()
            .flat_map(|(_, graph)| &graph.nodes)
            .filter_map(|node| match &node.config {
                NodeConfig::AudioDeviceInput(config) => Some(&config.selector),
                NodeConfig::AudioDeviceOutput(config) => Some(&config.selector),
                _ => None,
            })
            .collect();
        let devices = if selectors.is_empty() {
            Vec::new()
        } else {
            self.graph_resources
                .device_inventory(&selectors)
                .map_err(invalid)?
        };
        let compiler = self.factory.compiler();
        let composed = compose_catalog(
            &documents,
            CatalogExecutionContext {
                revision: document.revision,
                compile: context,
                compiler: &compiler,
                devices: &devices,
                applications: &NativeIdentities(self.graph_resources.as_ref()),
            },
        )
        .map_err(composition_error)?;
        self.validate_required_endpoint(document)?;
        Ok(composed)
    }

    pub(super) fn prepare_execution(
        &self,
        document: &GraphDocument,
        catalog: Option<&Catalog>,
        context: CompileContext,
    ) -> Result<PreparedExecution, ProtocolError> {
        // Production callers retain `mutation` through preparation and submission. With
        // no pending plan, the audio thread cannot retire a reuse owner while we prepare.
        let revisions = self.exchange.revisions();
        if revisions.active != revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphExchangeBusy,
                parameters: serde_json::json!({
                    "activeRevision": revisions.active,
                    "proposedRevision": document.revision,
                }),
            });
        }
        let composed = self.compose_execution(document, catalog, context)?;
        let captures = composed_capture_manifest(&composed)?;
        let endpoints =
            virtual_endpoint_specs(&composed.document, self.endpoint_backend.capabilities())?;
        let processor = self
            .factory
            .prepare(&composed.execution)
            .map_err(prepare_error)?;
        validate_endpoint_bindings(processor.plan(), &endpoints)?;
        let controls = std::sync::Arc::clone(processor.plan().controls());
        for intent in &composed.playback_intents {
            if controls.snapshot(&intent.mask_node_id).is_none()
                || !composed
                    .native_devices
                    .contains_key(&intent.forwarding_endpoint)
                || composed.native_sources.get(&intent.source_id) != Some(&intent.capture_node_id)
            {
                return Err(invalid(
                    "application playback intent is not bound to this plan",
                ));
            }
        }
        let prepared_native = composed
            .native_control_bindings
            .keys()
            .filter(|id| {
                composed
                    .document
                    .nodes
                    .iter()
                    .any(|node| &node.id == *id && node.control.enabled)
            })
            .cloned()
            .collect();
        Ok(PreparedExecution {
            processor,
            document: composed.document,
            endpoints: endpoints.into_iter().map(|(_, spec)| spec).collect(),
            playback_intents: composed.playback_intents.clone(),
            view: ExecutionView {
                origins: composed.node_origins,
                captures,
                playback_intents: composed.playback_intents,
                controls,
                native_controls: composed.native_control_bindings,
                prepared_native,
            },
        })
    }

    pub(super) fn current_capture_manifest(
        &self,
    ) -> Result<ApplicationCaptureManifest, ProtocolError> {
        // Native restore can hold the writer lock while CoreAudio is unavailable. A manifest
        // read must not join that wait or retry native identity resolution itself.
        let _mutation = self.mutation.try_lock().ok_or_else(|| ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({
                "resource":"application_capture_manifest",
                "reason":"graph_mutation_in_progress",
            }),
        })?;
        let revision = self.exchange.plan_revisions().submitted;
        let views = self.execution_views.read();
        // Native adapters start with an already compiled empty processor before
        // any graph submission publishes an execution view. It owns no captures.
        if views.is_empty() {
            let document = self.graph.read();
            let plans = self.exchange.plan_revisions();
            if document.nodes.is_empty()
                && document.edges.is_empty()
                && document.revision == plans.active
                && plans.active == plans.submitted
            {
                return Ok(ApplicationCaptureManifest {
                    protocol_version: PROTOCOL_VERSION,
                    revision: self.exchange.revisions().submitted,
                    sources: Vec::new(),
                });
            }
        }
        let view = views.get(&revision).ok_or_else(|| ProtocolError {
            code: ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({
                "resource":"application_capture_manifest",
                "reason":"execution_snapshot_not_published",
            }),
        })?;
        let mut manifest = view.captures.clone();
        manifest.revision = self.exchange.revisions().submitted;
        manifest.sources.retain(|source| {
            view.controls
                .snapshot(&NodeId::from(source.node_id.as_str()))
                .is_some_and(|(control, _)| control.enabled)
        });
        apply_playback_intents(&mut manifest, &view.playback_intents, |id| {
            view.controls.snapshot(id).map(|(control, _)| control)
        })?;
        Ok(manifest)
    }

    pub(super) fn preview_capture_manifest(
        &self,
        request: GraphApplyRequest,
    ) -> Result<ApplicationCaptureManifest, ProtocolError> {
        let _mutation = self.mutation.lock();
        let revisions = self.exchange.revisions();
        if request.document.revision <= revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({"submittedRevision": revisions.submitted}),
            });
        }
        if revisions.active != revisions.submitted {
            return Err(ProtocolError {
                code: ErrorCode::GraphExchangeBusy,
                parameters: serde_json::json!({"activeRevision": revisions.active}),
            });
        }
        let mut document = request.document;
        let mut catalog = self.catalog_candidate(&document);
        if let Some(candidate) = &mut catalog {
            self.reconcile_catalog_devices(candidate)?;
            document = candidate.active_document().clone();
        }
        let composed = self.compose_execution(&document, catalog.as_ref(), *self.context.read())?;
        composed_capture_manifest(&composed)
    }
}

pub(crate) fn composed_capture_manifest(
    composed: &ComposedGraph,
) -> Result<ApplicationCaptureManifest, ProtocolError> {
    let mut manifest = capture_manifest(&composed.document)?;
    apply_playback_intents(&mut manifest, &composed.playback_intents, |id| {
        composed
            .document
            .nodes
            .iter()
            .find(|node| &node.id == id)
            .map(|node| node.control)
    })?;
    Ok(manifest)
}

pub(super) fn apply_playback_intents(
    manifest: &mut ApplicationCaptureManifest,
    intents: &[crate::graph_composition::ApplicationPlaybackIntent],
    mut control: impl FnMut(&NodeId) -> Option<mixrelay_audio_graph::NodeControl>,
) -> Result<(), ProtocolError> {
    for source in &mut manifest.sources {
        source.request.source_playback = mixrelay_audio_io::SourcePlaybackPolicy::Preserve;
        source.request.forwarding_endpoint = None;
        for intent in intents
            .iter()
            .filter(|intent| intent.capture_node_id.as_str() == source.node_id)
        {
            if !control(&intent.mask_node_id).is_some_and(|value| value.enabled && !value.muted) {
                continue;
            }
            if source
                .request
                .forwarding_endpoint
                .as_ref()
                .is_some_and(|id| id != &intent.forwarding_endpoint)
            {
                return Err(invalid(
                    "one application source cannot suppress different forwarding endpoints",
                ));
            }
            source.request.source_playback =
                mixrelay_audio_io::SourcePlaybackPolicy::MuteWhileCaptured;
            source.request.forwarding_endpoint = Some(intent.forwarding_endpoint.clone());
        }
    }
    Ok(())
}

pub(crate) fn capture_manifest(
    document: &GraphDocument,
) -> Result<ApplicationCaptureManifest, ProtocolError> {
    let sources = document
        .nodes
        .iter()
        .filter(|node| node.control.enabled)
        .filter_map(|node| match &node.config {
            NodeConfig::ApplicationCapture(request) => Some((node, request)),
            _ => None,
        })
        .map(|(node, request)| {
            // The current native capture transport uses a bounded source-token ABI. Validate
            // the final canonical identity, not an authored local ID before namespacing.
            if node.id.as_str().len() > 127 {
                return Err(invalid(
                    "canonical application capture identity exceeds native capacity",
                ));
            }
            Ok(ApplicationCaptureSource {
                node_id: node.id.as_str().to_owned(),
                request: request.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ApplicationCaptureManifest {
        protocol_version: PROTOCOL_VERSION,
        revision: document.revision,
        sources,
    })
}

fn invalid(reason: impl std::fmt::Display) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::GraphInvalid,
        parameters: serde_json::json!({"reason": reason.to_string()}),
    }
}

fn composition_error(error: CompositionError) -> ProtocolError {
    match error {
        CompositionError::Compile(error) => ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({"issues":error.issues()}),
        },
        CompositionError::Invalid(reason) => invalid(reason),
    }
}

#[cfg(test)]
#[path = "execution_control_tests.rs"]
mod tests;
