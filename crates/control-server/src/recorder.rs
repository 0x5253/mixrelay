use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError, RecorderControlRequest};

use crate::{
    ControlState,
    reply::{catalog_task, reply},
};

pub(super) async fn status(
    State(state): State<ControlState>,
    Path((graph_id, node_id)): Path<(String, String)>,
) -> Response {
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state.backend.recorder_status(&graph_id, &node_id),
        )
    })
    .await
}

pub(super) async fn control(
    State(state): State<ControlState>,
    Path((graph_id, node_id)): Path<(String, String)>,
    request: Result<Json<RecorderControlRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ProtocolError {
                    code: ErrorCode::RecorderFailed,
                    parameters: serde_json::json!({"reason": error.body_text()}),
                }),
            )
                .into_response();
        }
    };
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state.backend.control_recorder(&graph_id, &node_id, request),
        )
    })
    .await
}

#[cfg(test)]
mod tests;
