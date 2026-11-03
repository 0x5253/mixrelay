use super::{ControlState, catalog_task, protocol_error_status};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, ProtocolError, WorkspaceRequest};

pub(super) async fn preview(
    State(state): State<ControlState>,
    Json(request): Json<WorkspaceRequest>,
) -> Response {
    catalog_task(
        move || match state.backend.preview_workspace_file(request) {
            Ok(response) => Json(response).into_response(),
            Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
        },
    )
    .await
}

pub(super) async fn operate(
    State(state): State<ControlState>,
    request: Result<Json<WorkspaceRequest>, JsonRejection>,
) -> Response {
    let request = match request {
        Ok(Json(request)) => request,
        Err(error) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({"reason":error.body_text()}),
                }),
            )
                .into_response();
        }
    };
    catalog_task(move || match state.backend.workspace_file(request) {
        Ok(response) => Json(response).into_response(),
        Err(error) => (protocol_error_status(error.code), Json(error)).into_response(),
    })
    .await
}
