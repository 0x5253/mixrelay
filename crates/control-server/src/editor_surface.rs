use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use mixrelay_control_protocol::{ErrorCode, ProcessorEditorInput};

use crate::{ControlState, reply::blocking_reply};

const TASK_FAILED: &str = "editor surface task failed";

pub(super) async fn frame(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
) -> Response {
    blocking_reply(
        StatusCode::OK,
        ErrorCode::ControlUnavailable,
        TASK_FAILED,
        move || state.backend.processor_editor_frame(&node_id),
    )
    .await
}

pub(super) async fn input(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
    Json(input): Json<ProcessorEditorInput>,
) -> Response {
    blocking_reply(
        StatusCode::OK,
        ErrorCode::ControlUnavailable,
        TASK_FAILED,
        move || state.backend.processor_editor_input(&node_id, input),
    )
    .await
}
