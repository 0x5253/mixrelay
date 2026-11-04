use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use mixrelay_control_protocol::{ErrorCode, WorkspaceRequest};

use crate::{
    ControlState,
    reply::{catalog_task, reply, unprocessable},
};

pub(super) async fn preview(
    State(state): State<ControlState>,
    Json(request): Json<WorkspaceRequest>,
) -> Response {
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state.backend.preview_workspace_file(request),
        )
    })
    .await
}

pub(super) async fn operate(
    State(state): State<ControlState>,
    request: Result<Json<WorkspaceRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    catalog_task(move || reply(StatusCode::OK, state.backend.workspace_file(request))).await
}
