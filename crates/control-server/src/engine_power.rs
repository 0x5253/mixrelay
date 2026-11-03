use crate::{ControlState, protocol_error_status};

pub(super) async fn preview(State(state): State<ControlState>) -> Response {
    match tokio::task::spawn_blocking(move || state.backend.preview_engine_start()).await {
        Ok(Ok(response)) => Json(response).into_response(),
        Ok(Err(error)) => (protocol_error_status(error.code), Json(error)).into_response(),
        Err(error) => {
            tracing::error!(%error, "engine power preflight failed");
            crate::operations::task_failure(
                ErrorCode::ControlUnavailable,
                "engine power preflight failed",
            )
        }
    }
}
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{EnginePowerRequest, ErrorCode};

pub(super) async fn set(
    State(state): State<ControlState>,
    Json(request): Json<EnginePowerRequest>,
) -> Response {
    match tokio::task::spawn_blocking(move || state.backend.set_engine_power(request)).await {
        Ok(Ok(response)) => (StatusCode::ACCEPTED, Json(response)).into_response(),
        Ok(Err(mut error)) => {
            error.parameters["powerOutcome"] = serde_json::json!("not_applied");
            (protocol_error_status(error.code), Json(error)).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "engine power control task failed");
            crate::operations::task_failure(
                ErrorCode::ControlUnavailable,
                "engine power control task failed",
            )
        }
    }
}
