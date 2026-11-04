//! External processors: the catalog, parameters, native editors and failure retry.
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use mixrelay_control_protocol::{
    ErrorCode, ExternalProcessorCatalog, ProcessorEditorRequest, ProcessorParameterUpdate,
    ProcessorRetryRequest, ProcessorScanRequest,
};

use crate::{
    ControlState,
    reply::{blocking_reply, reply, unprocessable},
};

pub(crate) async fn catalog(State(state): State<ControlState>) -> Json<ExternalProcessorCatalog> {
    Json(state.backend.processors())
}

pub(crate) async fn rescan(
    State(state): State<ControlState>,
    request: Result<Json<ProcessorScanRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    reply(StatusCode::OK, state.backend.rescan_processors(request))
}

pub(crate) async fn set_parameter(
    State(state): State<ControlState>,
    Path((node_id, parameter_id)): Path<(String, u32)>,
    update: Result<Json<ProcessorParameterUpdate>, JsonRejection>,
) -> Response {
    let Json(update) = match update {
        Ok(update) => update,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    // Accepted parameter updates also checkpoint durable graph state. Keep filesystem work
    // off Tokio's I/O workers; the backend still publishes the native value before saving.
    blocking_reply(
        StatusCode::OK,
        ErrorCode::ControlUnavailable,
        "processor parameter control task failed",
        move || {
            state
                .backend
                .set_processor_parameter(&node_id, parameter_id, update)
        },
    )
    .await
}

pub(crate) async fn editor_status(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
) -> Response {
    reply(
        StatusCode::OK,
        state.backend.processor_editor_status(&node_id),
    )
}

pub(crate) async fn set_editor(
    State(state): State<ControlState>,
    Path(node_id): Path<String>,
    request: Result<Json<ProcessorEditorRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    // Closing includes a bounded vendor state export and durable filesystem write.
    // Do not stall meter/status HTTP requests on the asynchronous I/O executor.
    blocking_reply(
        StatusCode::OK,
        ErrorCode::ControlUnavailable,
        "processor editor control task failed",
        move || state.backend.set_processor_editor(&node_id, request),
    )
    .await
}

pub(crate) async fn retry_failed(
    State(state): State<ControlState>,
    request: Result<Json<ProcessorRetryRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    blocking_reply(
        StatusCode::OK,
        ErrorCode::ControlUnavailable,
        "processor retry task failed",
        move || state.backend.retry_failed_processor(request),
    )
    .await
}

#[cfg(test)]
mod tests;
