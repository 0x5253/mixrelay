//! Application capture manifests: the current one and a preview for a candidate graph.
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use mixrelay_control_protocol::{ErrorCode, GraphApplyRequest};

use crate::{
    ControlState,
    reply::{catalog_task, reply, unprocessable},
};

pub(crate) async fn manifest(State(state): State<ControlState>) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.application_capture_manifest())).await
}

pub(crate) async fn preview(
    State(state): State<ControlState>,
    request: Result<Json<GraphApplyRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state.backend.preview_application_capture(request),
        )
    })
    .await
}

#[cfg(test)]
mod tests;
