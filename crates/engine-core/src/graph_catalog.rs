//! Durable graph documents are owned by the engine, not by a sidebar component.
//! The controller's mutation lock serializes catalog edits with graph/session publication.
//! Selection only changes the edited view; live processor instances belong to the composed
//! catalog and are not destroyed or snapshotted merely because another graph is selected.

use std::{collections::BTreeSet, io::Read, path::PathBuf};

use mixrelay_control_protocol::{
    GraphCatalogAction, GraphCatalogEntry, GraphCatalogRequest, GraphCatalogResponse, GraphLayout,
};
use serde::{Deserialize, Serialize};

use super::{
    EngineGraphController, ErrorCode, GraphDocument, NodeConfig, PROTOCOL_VERSION, ProtocolError,
};
use crate::document_files::{DocumentFiles, StagedWrite, stage_json_limited};

const MAX_BYTES: u64 = 16 * 1024 * 1024;

fn document_write_error(error: &std::io::Error) -> ProtocolError {
    if error.kind() == std::io::ErrorKind::InvalidData {
        invalid(&error.to_string())
    } else {
        unavailable(&error.to_string())
    }
}
const MAX_GRAPHS: usize = 256;

#[path = "graph_layout.rs"]
mod layout;

#[path = "graph_native_state.rs"]
mod native_state;

#[path = "graph_live_state.rs"]
pub(crate) mod live_state;

#[path = "graph_restore.rs"]
mod restore;
pub(super) use restore::CatalogRecall;

#[path = "device_catalog.rs"]
mod devices;

mod deletion_history;
mod saved_graphs;
mod state_assets;
mod storage;
pub(super) use storage::CatalogStorage;
use storage::Persistence;
mod workspace;

use saved_graphs::SavedGraphs;

#[derive(Clone, Deserialize)]
#[serde(try_from = "CatalogFile")]
pub(super) struct Catalog {
    version: u32,
    revision: u64,
    graphs: SavedGraphs,
    deleted: Vec<deletion_history::Deletion>,
    restored: Vec<deletion_history::Deletion>,
}

/// Saved catalog shape: the selection is stored by graph identity.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogFile {
    version: u32,
    revision: u64,
    active_id: String,
    graphs: Vec<SavedGraph>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogFileView<'a> {
    version: u32,
    revision: u64,
    active_id: &'a str,
    graphs: Vec<&'a SavedGraph>,
}

impl TryFrom<CatalogFile> for Catalog {
    type Error = &'static str;

    fn try_from(file: CatalogFile) -> Result<Self, Self::Error> {
        let graphs =
            SavedGraphs::from_list(file.graphs, &file.active_id).ok_or(INVALID_CATALOG_IDENTITY)?;
        Ok(Self {
            version: file.version,
            revision: file.revision,
            graphs,
            deleted: Vec::new(),
            restored: Vec::new(),
        })
    }
}

impl Serialize for Catalog {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        CatalogFileView {
            version: self.version,
            revision: self.revision,
            active_id: self.selected_id(),
            graphs: self.graphs.iter().collect(),
        }
        .serialize(serializer)
    }
}

const INVALID_CATALOG_IDENTITY: &str = "invalid graph catalog identity, name or version";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedGraph {
    id: String,
    name: String,
    document: GraphDocument,
    layout_revision: u64,
    layout: GraphLayout,
}

const UNTITLED: &str = "Untitled Graph";

impl SavedGraph {
    fn new(id: String, name: String, document: GraphDocument) -> Self {
        Self {
            id,
            name,
            document,
            layout_revision: 0,
            layout: GraphLayout::default(),
        }
    }
}

pub(super) fn invalid(reason: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::GraphInvalid,
        parameters: serde_json::json!({"reason": reason.to_string()}),
    }
}

pub(super) fn unavailable(reason: &str) -> ProtocolError {
    ProtocolError {
        code: ErrorCode::ControlUnavailable,
        parameters: serde_json::json!({"reason": reason.to_string(), "resource": "graph_catalog"}),
    }
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
}

impl Catalog {
    pub(super) fn selected_id(&self) -> &str {
        &self.graphs.active().id
    }

    pub(super) fn documents_mut(&mut self) -> impl Iterator<Item = (&str, &mut GraphDocument)> {
        self.graphs
            .iter_mut()
            .map(|graph| (graph.id.as_str(), &mut graph.document))
    }
    pub(super) fn active_document(&self) -> &GraphDocument {
        &self.active().document
    }

    pub(super) fn documents(&self) -> impl Iterator<Item = (&str, &GraphDocument)> {
        self.graphs
            .iter()
            .map(|graph| (graph.id.as_str(), &graph.document))
    }

    pub(super) fn control_documents(
        &self,
    ) -> impl Iterator<Item = (&str, &str, &GraphDocument, &GraphLayout)> {
        self.graphs.iter().map(|graph| {
            (
                graph.id.as_str(),
                graph.name.as_str(),
                &graph.document,
                &graph.layout,
            )
        })
    }

    fn imported(document: GraphDocument) -> Self {
        Self {
            version: 1,
            revision: 1,
            deleted: Vec::new(),
            restored: Vec::new(),
            graphs: SavedGraphs::single(SavedGraph::new(
                "graph-1".into(),
                UNTITLED.into(),
                document,
            )),
        }
    }

    fn load(files: &dyn DocumentFiles, path: &std::path::Path) -> Result<Option<Self>, String> {
        let file = match files.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("graph catalog exceeds size limit".into());
        }
        let mut value: Self = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        value.validate()?;
        for graph in value.graphs.iter_mut() {
            restore::require_pid_reselection(&mut graph.document);
        }
        Ok(Some(value))
    }

    fn validate(&self) -> Result<(), String> {
        let value = self;
        let mut ids = BTreeSet::new();
        if value.version != 1
            || value.graphs.len() > MAX_GRAPHS
            || !value.graphs.iter().all(|graph| {
                valid_name(&graph.name)
                    && !graph.id.is_empty()
                    && graph.id.len() <= 256
                    && ids.insert(graph.id.clone())
                    && layout::validate(&graph.layout).is_ok()
            })
        {
            return Err(INVALID_CATALOG_IDENTITY.into());
        }
        state_assets::referenced(self).map_err(|error| {
            error
                .parameters
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("invalid processor state reference")
                .to_owned()
        })?;
        Ok(())
    }

    fn active(&self) -> &SavedGraph {
        self.graphs.active()
    }

    /// Startup is a new publication, not a new revision namespace. Include
    /// inactive graphs so selecting another saved view cannot reuse an old ID.
    fn next_audio_revision(&self, submitted: u64) -> Option<u64> {
        self.graphs
            .iter()
            .map(|graph| graph.document.revision)
            .fold(submitted, u64::max)
            .checked_add(1)
    }

    pub(super) fn update_document(&mut self, document: GraphDocument) {
        self.graphs.active_mut().document = document;
    }

    fn response(&self) -> GraphCatalogResponse {
        GraphCatalogResponse {
            protocol_version: PROTOCOL_VERSION,
            catalog_revision: self.revision,
            active_id: self.selected_id().to_owned(),
            graphs: self
                .graphs
                .iter()
                .map(|graph| GraphCatalogEntry {
                    id: graph.id.clone(),
                    name: graph.name.clone(),
                })
                .collect(),
            document: self.active().document.clone(),
            layout_revision: self.active().layout_revision,
            layout: self.active().layout.clone(),
            persistence_error: None,
        }
    }
}

impl EngineGraphController {
    #[must_use]
    pub fn with_unavailable_graph_catalog(self, error: &std::io::Error) -> Self {
        self.catalog.initialize(
            PathBuf::new(),
            Persistence::SessionOnly,
            Err(error.to_string()),
        );
        self
    }

    /// Restores the explicitly selected catalog before the audio worker starts.
    #[must_use]
    pub fn with_graph_catalog_path(mut self, path: PathBuf) -> Self {
        // A paused engine restores its graph without claiming audio.
        self.power.restore(&path);
        let loaded = Catalog::load(self.documents.as_ref(), &path);
        self.processor_files
            .select_namespace(&path.with_extension("states"));
        match loaded {
            Ok(None) => {
                // Import the already prepared graph once. Never initialize an existing file.
                let value = Catalog::imported(self.graph());
                let saved = stage_json_limited(self.documents.as_ref(), &path, &value, MAX_BYTES)
                    .and_then(StagedWrite::commit);
                self.catalog.initialize(
                    path,
                    Persistence::Durable,
                    saved.map(|()| value).map_err(|error| error.to_string()),
                );
            }
            Ok(Some(value)) => {
                let document = value.active().document.clone();
                self.catalog
                    .initialize(path, Persistence::Durable, Ok(value));
                // The editable document is durable user data, not the bootstrap audio
                // processor. Failed native startup must never substitute that processor's graph.
                *self.graph.write() = document;
                if let Err(error) = self.restore_catalog_locked() {
                    self.defer_catalog_restore(error);
                }
            }
            Err(error) => {
                self.catalog
                    .initialize(path, Persistence::Durable, Err(error));
            }
        }
        self.refresh_control_snapshot();
        self
    }

    pub(super) fn catalog_candidate(&self, document: &GraphDocument) -> Option<Catalog> {
        self.catalog.candidate(document)
    }

    pub(super) fn stage_catalog(
        &self,
        candidate: Option<&Catalog>,
    ) -> Result<Option<StagedWrite>, ProtocolError> {
        if self.catalog.writes_disabled() {
            return Ok(None);
        }
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        self.catalog.stage(self.documents.as_ref(), candidate, || {
            self.processor_files
                .snapshot()
                .map_err(|error| unavailable(&error.to_string()))
        })
    }

    pub(super) fn commit_catalog_memory(&self, candidate: Option<Catalog>) {
        self.replace_catalog_memory(candidate);
        self.refresh_control_snapshot();
    }

    /// Updates authored selection before a prepared plan can become active.
    pub(super) fn replace_catalog_memory(&self, candidate: Option<Catalog>) {
        self.catalog.replace(candidate);
    }

    pub(super) fn finish_catalog_write(&self, staged: Option<StagedWrite>) {
        self.catalog.finish(staged);
    }

    /// Called with the controller mutation lock after a parameter was accepted natively.
    /// Disk failures are durability warnings, not failed audio edits. Keep memory current so
    /// a later successful edit also persists every previously accepted parameter ID.
    pub(super) fn persist_parameter_document(&self, document: &GraphDocument) -> Option<String> {
        let candidate = self.catalog.inspect(|value| match value? {
            Ok(value) => {
                let mut candidate = value.clone();
                candidate.update_document(document.clone());
                Some(Ok(candidate))
            }
            Err(error) => Some(Err(format!("graph catalog is unavailable: {error}"))),
        })?;
        let candidate = match candidate {
            Ok(candidate) => candidate,
            Err(error) => {
                self.catalog.record_warning(Some(error.clone()));
                return Some(error);
            }
        };
        let result = self
            .stage_catalog(Some(&candidate))
            .map_err(|error| {
                error
                    .parameters
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("graph catalog staging failed")
                    .to_owned()
            })
            .and_then(|staged| {
                staged.map_or(Ok(()), |write| {
                    write.commit().map_err(|error| error.to_string())
                })
            });
        self.commit_catalog_memory(Some(candidate));
        let error = result.err();
        self.catalog.record_warning(error.clone());
        error
    }

    pub(super) fn read_catalog(&self) -> Result<GraphCatalogResponse, ProtocolError> {
        // Read one committed document/layout snapshot; native preparation owns mutation,
        // but must never prevent the UI from displaying durable user data.
        let mut response = self.catalog.read(Catalog::response)?;
        response.persistence_error = self.graph_persistence_warning();
        Ok(response)
    }

    pub(super) fn graph_persistence_warning(&self) -> Option<String> {
        self.catalog
            .warning()
            .or_else(|| self.power.warning())
            .or_else(|| self.node_control_warning.lock().clone())
    }

    pub(super) fn read_catalog_document(&self, id: &str) -> Result<GraphDocument, ProtocolError> {
        self.catalog.read(|value| {
            value
                .graphs
                .iter()
                .find(|graph| graph.id == id)
                .map(|graph| graph.document.clone())
                .ok_or_else(|| invalid("graph does not exist"))
        })?
    }

    // Caller holds mutation across document validation and its control operation.
    pub(super) fn catalog_document_locked(&self, id: &str) -> Result<GraphDocument, ProtocolError> {
        self.catalog.read(|value| {
            if id == value.selected_id() {
                return Ok(self.graph.read().clone());
            }
            value
                .graphs
                .iter()
                .find(|graph| graph.id == id)
                .map(|graph| graph.document.clone())
                .ok_or_else(|| invalid("graph does not exist"))
        })?
    }

    pub(super) fn mutate_catalog(
        &self,
        request: GraphCatalogRequest,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        let mut value = self.catalog.read(Clone::clone)?;
        let revisions = self.exchange.revisions();
        if request.expected_revision != revisions.submitted
            || revisions.active != revisions.submitted
            || request.expected_catalog_revision != value.revision
        {
            return Err(ProtocolError {
                code: ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({"submittedRevision": revisions.submitted, "catalogRevision": value.revision}),
            });
        }
        if matches!(&request.action, GraphCatalogAction::Select { id } if id == value.selected_id())
        {
            return self.current_catalog_response();
        }
        if match &request.action {
            GraphCatalogAction::UndoDelete => value.deleted.is_empty(),
            GraphCatalogAction::RedoDelete => value.restored.is_empty(),
            _ => false,
        } {
            return self.current_catalog_response();
        }
        let next_catalog_revision = value
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("catalog revision overflow"))?;
        value.update_document(self.graph.read().clone());
        let removes_execution = matches!(
            request.action,
            GraphCatalogAction::Delete { .. }
                | GraphCatalogAction::UndoDelete
                | GraphCatalogAction::RedoDelete
        );
        value.apply_action(request.action, revisions.submitted, next_catalog_revision)?;
        value.revision = next_catalog_revision;
        if removes_execution {
            return self.publish_catalog_removal(value, revisions.submitted);
        }
        let mut document = value.active_document().clone();
        document.revision = revisions.submitted;
        value.update_document(document.clone());
        if let Some(staged) = self.stage_catalog(Some(&value))? {
            staged
                .commit()
                .map_err(|error| unavailable(&error.to_string()))?;
        }
        *self.graph.write() = document;
        self.catalog.record_warning(None);
        self.commit_catalog_memory(Some(value.clone()));
        let mut response = value.response();
        response.persistence_error = self.graph_persistence_warning();
        Ok(response)
    }

    fn current_catalog_response(&self) -> Result<GraphCatalogResponse, ProtocolError> {
        let mut response = self.catalog.read(Catalog::response)?;
        response.document = self.graph.read().clone();
        response.persistence_error = self.graph_persistence_warning();
        Ok(response)
    }

    fn publish_catalog_removal(
        &self,
        mut candidate: Catalog,
        revision: u64,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        let mut document = candidate.active_document().clone();
        document.revision = revision
            .checked_add(1)
            .ok_or_else(|| invalid("graph revision overflow"))?;
        if candidate.selected_id() == self.view_graph_id() {
            self.preserve_live_processor_state(&mut document, &[])?;
        }
        candidate.update_document(document);
        self.reconcile_catalog_devices(&mut candidate)?;
        // The selected identity may have changed. Preserve providers by catalog owner,
        // not by comparing graph-local node IDs against the previously selected graph.
        self.preserve_catalog_provider_state(&mut candidate)?;
        self.publish_graph_locked(candidate.active_document().clone(), Some(candidate))?;
        reuse.commit();
        self.current_catalog_response()
    }
}

impl Catalog {
    fn apply_action(
        &mut self,
        action: GraphCatalogAction,
        next_revision: u64,
        next_catalog_revision: u64,
    ) -> Result<bool, ProtocolError> {
        let mut switch = true;
        match action {
            GraphCatalogAction::Create { name } => {
                if !valid_name(&name) || self.graphs.len() >= MAX_GRAPHS {
                    return Err(invalid("invalid graph name or catalog full"));
                }
                let id = format!("graph-{next_catalog_revision}");
                if self.graphs.iter().any(|graph| graph.id == id) {
                    return Err(invalid("graph identity collision"));
                }
                self.graphs.push_selected(SavedGraph::new(
                    id,
                    name.trim().into(),
                    GraphDocument::new(next_revision, vec![], vec![]),
                ));
            }
            GraphCatalogAction::Select { id } => {
                if !self.graphs.select(&id) {
                    return Err(invalid("graph does not exist"));
                }
            }
            GraphCatalogAction::Rename { id, name } => {
                if !valid_name(&name) {
                    return Err(invalid("invalid graph name"));
                }
                let graph = self
                    .graphs
                    .iter_mut()
                    .find(|graph| graph.id == id)
                    .ok_or_else(|| invalid("graph does not exist"))?;
                graph.name = name.trim().into();
                switch = false;
            }
            GraphCatalogAction::Delete { id } => {
                self.delete_graph(&id, next_revision, next_catalog_revision)?;
                self.restored.clear();
            }
            GraphCatalogAction::UndoDelete => self.undo_deletion()?,
            GraphCatalogAction::RedoDelete => {
                let deletion = self
                    .restored
                    .pop()
                    .ok_or_else(|| invalid("no graph deletion to redo"))?;
                self.delete_graph(&deletion.graph.id, next_revision, next_catalog_revision)?;
            }
        }
        Ok(switch)
    }
}
