use serde::{Deserialize, Serialize};

/// One actual native editor image. Coordinates use logical pixels, not image pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorEditorFrame {
    pub instance_id: String,
    pub session: u64,
    pub width: u32,
    pub height: u32,
    pub png_base64: String,
}

/// Pointer transitions for an isolated native editor, ordered by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorPointerKind {
    Down,
    Move,
    Up,
    Cancel,
}

/// Input is fenced to the worker and open-editor session that produced the image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessorEditorInput {
    pub instance_id: String,
    pub session: u64,
    pub kind: EditorPointerKind,
    pub x: f64,
    pub y: f64,
    pub modifiers: u8,
}
