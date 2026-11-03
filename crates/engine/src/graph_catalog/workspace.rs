//! Portable workspace bundles contain authored catalog data and immutable processor state.
use std::{
    io::{self, Read},
    path::Path,
};

use crate::document_files::DocumentFiles;
use mixrelay_control_protocol::{GraphCatalogResponse, WorkspaceAction, WorkspaceRequest};
use mixrelay_processor_state::{StateError, StateRef, preview_bundle, read_bundle, write_bundle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Catalog, EngineGraphController, GraphDocument, MAX_BYTES, NodeConfig, ProtocolError, invalid,
    restore, state_assets, unavailable,
};

mod preview;
mod rates;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceFile {
    format: String,
    version: u32,
    catalog: Catalog,
}

fn file_path(path: &str) -> Result<&Path, ProtocolError> {
    let path = Path::new(path);
    if !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mix"))
    {
        return Err(invalid("workspace path must be an absolute .mix filename"));
    }
    Ok(path)
}

fn load_fingerprinted(
    files: &dyn DocumentFiles,
    path: &Path,
) -> Result<(Catalog, String, Vec<StateRef>), ProtocolError> {
    let file = files
        .open(path)
        .map_err(|error| unavailable(&error.to_string()))?;
    let mut reader = FingerprintedReader::new(file);
    let (manifest, bundled_refs) =
        preview_bundle(&mut reader).map_err(|error| bundle_read_error(&error))?;
    reader
        .require_eof()
        .map_err(|error| invalid(&format!("invalid MixRelay workspace bundle: {error}")))?;
    let digest = reader.digest();
    let (catalog, references) = parse_manifest(&manifest, &bundled_refs)?;
    Ok((catalog, digest, references))
}

fn parse_manifest(
    manifest: &[u8],
    bundled_refs: &[StateRef],
) -> Result<(Catalog, Vec<StateRef>), ProtocolError> {
    if manifest.len() as u64 > MAX_BYTES {
        return Err(invalid("workspace manifest exceeds size limit"));
    }
    let mut file: WorkspaceFile = serde_json::from_slice(manifest)
        .map_err(|_| invalid("invalid MixRelay workspace manifest"))?;
    if file.format != "MixRelay" || file.version != 1 {
        return Err(invalid("unsupported MixRelay workspace format or version"));
    }
    file.catalog.validate().map_err(|error| invalid(&error))?;
    file.catalog.device_specs()?;
    let references = state_assets::referenced(&file.catalog)?;
    state_assets::require_exact(&references, bundled_refs)?;
    for (_, document) in file.catalog.documents_mut() {
        restore::require_pid_reselection(document);
    }
    Ok((file.catalog, references))
}

fn import_bundle(
    files: &dyn DocumentFiles,
    path: &Path,
    expected_digest: &str,
    expected_refs: &[StateRef],
    store: &dyn mixrelay_processor_state::StateBlobSink,
) -> Result<(), ProtocolError> {
    let file = files
        .open(path)
        .map_err(|error| unavailable(&error.to_string()))?;
    let mut reader = FingerprintedReader::new(file);
    let (manifest, bundled_refs) =
        read_bundle(&mut reader, store).map_err(|error| bundle_read_error(&error))?;
    reader
        .require_eof()
        .map_err(|error| invalid(&format!("invalid MixRelay workspace bundle: {error}")))?;
    let digest = reader.digest();
    if digest != expected_digest {
        return Err(invalid("workspace file changed during state import"));
    }
    let (_, references) = parse_manifest(&manifest, &bundled_refs)?;
    state_assets::require_exact(expected_refs, &references)?;
    Ok(())
}

fn bundle_read_error(error: &StateError) -> ProtocolError {
    if matches!(&error, StateError::Io(_)) {
        unavailable(&error.to_string())
    } else {
        invalid(&format!("invalid MixRelay workspace bundle: {error}"))
    }
}

struct FingerprintedReader<R> {
    inner: R,
    digest: Sha256,
}

impl<R: Read> FingerprintedReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn digest(&self) -> String {
        format!("{:x}", self.digest.clone().finalize())
    }

    fn require_eof(&mut self) -> io::Result<()> {
        let mut trailing = [0_u8; 1];
        if self.read(&mut trailing)? != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "workspace bundle contains trailing bytes",
            ));
        }
        Ok(())
    }
}

impl<R: Read> Read for FingerprintedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.digest.update(&buffer[..read]);
        Ok(read)
    }
}

impl EngineGraphController {
    pub(crate) fn workspace_operation(
        &self,
        request: WorkspaceRequest,
    ) -> Result<GraphCatalogResponse, ProtocolError> {
        let _mutation = self.mutation.lock();
        let revisions = self.exchange.revisions();
        let current = self
            .catalog_candidate(&self.graph.read())
            .ok_or_else(|| unavailable("graph catalog is unavailable"))?;
        if revisions.active != revisions.submitted {
            return Err(ProtocolError {
                code: super::ErrorCode::GraphExchangeBusy,
                parameters: serde_json::json!({}),
            });
        }
        if request.expected_revision != revisions.submitted
            || request.expected_catalog_revision != current.revision
        {
            return Err(ProtocolError {
                code: super::ErrorCode::GraphRevisionStale,
                parameters: serde_json::json!({}),
            });
        }
        match request.operation {
            WorkspaceAction::Save { path } => self.save_workspace(file_path(&path)?, current)?,
            WorkspaceAction::Open { path } => {
                let path = file_path(&path)?;
                let (candidate, digest, references) =
                    load_fingerprinted(self.documents.as_ref(), path)?;
                if request
                    .file_digest
                    .as_ref()
                    .is_some_and(|expected| expected != &digest)
                {
                    return Err(invalid("workspace file changed after capture preparation"));
                }
                let store = self
                    .processor_files
                    .snapshot()
                    .map_err(|error| unavailable(&error.to_string()))?;
                import_bundle(self.documents.as_ref(), path, &digest, &references, &store)?;
                self.replace_workspace(candidate, &current)?;
            }
            WorkspaceAction::New => self.replace_workspace(
                Catalog::imported(GraphDocument::new(0, vec![], vec![])),
                &current,
            )?,
        }
        // An explicit file operation opts this session into durable recall.
        // Failed file operations never replace a protected saved catalog.
        self.catalog.allow_persistence();
        let candidate = self.catalog_candidate(&self.graph.read());
        let staged = self.stage_catalog(candidate.as_ref())?;
        self.finish_catalog_write(staged);
        self.read_catalog()
    }

    fn save_workspace(&self, path: &Path, mut candidate: Catalog) -> Result<(), ProtocolError> {
        // Export does not publish the running catalog, even when the file write succeeds.
        let _reuse = self.processor_controls.reuse_transaction();
        for (id, document) in candidate.documents_mut() {
            let previous = document.clone();
            self.capture_background_graph((id, document), &previous, &mut |node, timeout| {
                self.processor_controls.snapshot_state(node, timeout)
            })?;
            for node in &mut document.nodes {
                if let NodeConfig::Provider(provider) = &mut node.config
                    && provider.ports.is_none()
                {
                    provider.ports = self.processor_catalog.ports(provider.type_id.as_str());
                }
            }
        }
        let references = state_assets::referenced(&candidate)?;
        let store = self
            .processor_files
            .snapshot()
            .map_err(|error| unavailable(&error.to_string()))?;
        let file = WorkspaceFile {
            format: "MixRelay".into(),
            version: 1,
            catalog: candidate,
        };
        let manifest =
            serde_json::to_vec(&file).map_err(|error| unavailable(&error.to_string()))?;
        if manifest.len() as u64 > MAX_BYTES {
            return Err(invalid("workspace manifest exceeds size limit"));
        }
        self.documents
            .stage(
                path,
                Box::new(|writer| {
                    write_bundle(writer, &manifest, &references, &store).map_err(io::Error::other)
                }),
            )
            .and_then(crate::document_files::StagedWrite::commit)
            .map_err(|error| super::document_write_error(&error))
    }

    fn replace_workspace(
        &self,
        mut candidate: Catalog,
        current: &Catalog,
    ) -> Result<(), ProtocolError> {
        let reuse = self.processor_controls.reuse_transaction();
        self.require_stopped_recorders(current)?;
        let revision = candidate
            .next_audio_revision(self.exchange.revisions().submitted)
            .ok_or_else(|| invalid("audio revision overflow"))?;
        candidate.revision = current
            .revision
            .max(candidate.revision)
            .checked_add(1)
            .ok_or_else(|| invalid("catalog revision overflow"))?;
        let layout_revision = current
            .graphs
            .iter()
            .chain(candidate.graphs.iter())
            .map(|graph| graph.layout_revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid("layout revision overflow"))?;
        for graph in candidate.graphs.iter_mut() {
            graph.document.revision = revision;
            graph.layout_revision = layout_revision;
        }
        // A file replaces authored state. Never merge old device definitions or live presets.
        self.processor_controls.invalidate_reuse();
        self.publish_workspace_rates(candidate)?;
        reuse.commit();
        Ok(())
    }

    fn require_stopped_recorders(&self, current: &Catalog) -> Result<(), ProtocolError> {
        for (graph_id, graph) in current.documents() {
            for node in &graph.nodes {
                let NodeConfig::Recorder { channels } = node.config else {
                    continue;
                };
                let id = crate::graph_composition::runtime_node_id(graph_id, &node.id);
                let key = mixrelay_audio_engine::recording_session_key(&id, channels);
                if matches!(
                    self.recorders.status(&key).state,
                    mixrelay_audio_io::recording::RecorderState::Recording
                        | mixrelay_audio_io::recording::RecorderState::Paused
                ) {
                    return Err(invalid("stop recording before replacing the workspace"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
fn load(files: &dyn DocumentFiles, path: &Path) -> Result<Catalog, ProtocolError> {
    load_fingerprinted(files, path).map(|(catalog, _, _)| catalog)
}
#[cfg(test)]
mod tests;
