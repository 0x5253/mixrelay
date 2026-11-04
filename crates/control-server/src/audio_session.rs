//! Audio session and virtual endpoint format changes.
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::Response,
};
use mixrelay_control_protocol::{
    ErrorCode, SessionReconfigureRequest, VirtualEndpointFormatRequest,
};

use crate::{
    ControlState,
    reply::{blocking_reply, unprocessable},
};

pub(crate) async fn reconfigure(
    State(state): State<ControlState>,
    request: Result<Json<SessionReconfigureRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::AudioSessionUnsupported, &rejection),
    };
    // Driver negotiation and plug-in preparation can block. Do not occupy a Tokio I/O worker.
    blocking_reply(
        StatusCode::ACCEPTED,
        ErrorCode::AudioSessionReconfigureFailed,
        "session control task failed",
        move || state.backend.reconfigure_session(request),
    )
    .await
}

pub(crate) async fn reconfigure_virtual_endpoint(
    State(state): State<ControlState>,
    request: Result<Json<VirtualEndpointFormatRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::AudioSessionUnsupported, &rejection),
    };
    blocking_reply(
        StatusCode::ACCEPTED,
        ErrorCode::AudioSessionReconfigureFailed,
        "virtual endpoint control task failed",
        move || state.backend.reconfigure_virtual_endpoint(request),
    )
    .await
}

#[cfg(test)]
mod tests;
