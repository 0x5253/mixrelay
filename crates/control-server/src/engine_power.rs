use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{EnginePowerRequest, ErrorCode};

use crate::{
    ControlState,
    operations::task_failure,
    reply::{failure, reply},
};

pub(super) async fn preview(State(state): State<ControlState>) -> Response {
    match tokio::task::spawn_blocking(move || state.backend.preview_engine_start()).await {
        Ok(result) => reply(StatusCode::OK, result),
        Err(error) => {
            tracing::error!(%error, "engine power preflight failed");
            task_failure(
                ErrorCode::ControlUnavailable,
                "engine power preflight failed",
            )
        }
    }
}

pub(super) async fn set(
    State(state): State<ControlState>,
    Json(request): Json<EnginePowerRequest>,
) -> Response {
    match tokio::task::spawn_blocking(move || state.backend.set_engine_power(request)).await {
        Ok(Ok(response)) => (StatusCode::ACCEPTED, Json(response)).into_response(),
        Ok(Err(mut error)) => {
            error.parameters["powerOutcome"] = serde_json::json!("not_applied");
            failure(error)
        }
        Err(error) => {
            tracing::error!(%error, "engine power control task failed");
            task_failure(
                ErrorCode::ControlUnavailable,
                "engine power control task failed",
            )
        }
    }
}
