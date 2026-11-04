//! The active graph: its document, publication status and application.
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use mixrelay_audio_graph::GraphDocument;
use mixrelay_control_protocol::{ErrorCode, GraphApplyRequest, GraphRuntimeStatus};

use crate::{
    ControlState,
    reply::{blocking_reply, unprocessable},
};

pub(crate) async fn document(State(state): State<ControlState>) -> Json<GraphDocument> {
    Json(state.backend.graph())
}

pub(crate) async fn status(State(state): State<ControlState>) -> Json<GraphRuntimeStatus> {
    Json(state.backend.graph_status())
}

pub(crate) async fn apply(
    State(state): State<ControlState>,
    request: Result<Json<GraphApplyRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    blocking_reply(
        StatusCode::ACCEPTED,
        ErrorCode::ControlUnavailable,
        "graph control task failed",
        move || state.backend.apply_graph_request(request),
    )
    .await
}

#[cfg(test)]
mod tests;
