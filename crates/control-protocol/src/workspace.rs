use serde::{Deserialize, Serialize};

/// Explicit file operations; automatic recovery storage is a separate engine concern.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceAction {
    /// Replace the workspace with an empty graph.
    New,
    /// Validate and load a complete workspace from an absolute `.mix` path.
    Open { path: String },
    /// Snapshot all graphs and atomically write an absolute `.mix` path.
    Save { path: String },
}

/// Guards file operations against concurrent graph publications and catalog selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceRequest {
    pub expected_revision: u64,
    pub expected_catalog_revision: u64,
    pub operation: WorkspaceAction,
    /// Exact file fingerprint returned by native capture preflight, when required.
    #[serde(default)]
    pub file_digest: Option<String>,
}

/// Detached capture preparation data; neither preview nor its digest publishes audio.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspacePreview {
    pub manifest: crate::ApplicationCaptureManifest,
    pub file_digest: Option<String>,
}
