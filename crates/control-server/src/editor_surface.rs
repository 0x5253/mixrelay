use super::{ControlState, protocol_error_status};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProcessorEditorInput, ProtocolError};

pub(super) async fn frame(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
) -> Response {
    reply(tokio::task::spawn_blocking(move || state.backend.processor_editor_frame(&node_id)).await)
}

pub(super) async fn input(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
    Json(input): Json<ProcessorEditorInput>,
) -> Response {
    reply(
        tokio::task::spawn_blocking(move || state.backend.processor_editor_input(&node_id, input))
            .await,
    )
}

fn reply<T: serde::Serialize>(
    result: Result<Result<T, ProtocolError>, tokio::task::JoinError>,
) -> Response {
    match result {
        Ok(Ok(value)) => (StatusCode::OK, Json(value)).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(_) => super::operations::task_failure(
            ErrorCode::ControlUnavailable,
            "editor surface task failed",
        ),
    }
}
