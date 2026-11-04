use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{ErrorCode, NodeControlRequest, ProtocolError};

use crate::{
    ControlState,
    reply::{catalog_task, failure, reply},
};

pub(super) async fn snapshot(State(state): State<ControlState>) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.controls_snapshot())).await
}

pub(super) async fn list(
    State(state): State<ControlState>,
    Path(graph_id): Path<String>,
) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.node_controls(&graph_id))).await
}

pub(super) async fn preview(
    State(state): State<ControlState>,
    Path((graph_id, node_id)): Path<(String, String)>,
    request: Result<Json<NodeControlRequest>, JsonRejection>,
) -> Response {
    let request = match decode(request) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    };
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state
                .backend
                .preview_node_control(&graph_id, &node_id, request),
        )
    })
    .await
}

pub(super) async fn update(
    State(state): State<ControlState>,
    Path((graph_id, node_id)): Path<(String, String)>,
    request: Result<Json<NodeControlRequest>, JsonRejection>,
) -> Response {
    let request = match decode(request) {
        Ok(request) => request,
        Err(error) => return (StatusCode::BAD_REQUEST, Json(error)).into_response(),
    };
    catalog_task(move || {
        match state
            .backend
            .update_node_control(&graph_id, &node_id, request)
        {
            Ok(response) => (StatusCode::ACCEPTED, Json(response)).into_response(),
            Err(mut error) => {
                // A returned backend error is definitive. A crashed/lost task is not;
                // desktop capture ownership must keep waiting in that uncertain case.
                error.parameters["nodeControlOutcome"] = serde_json::json!("not_applied");
                failure(error)
            }
        }
    })
    .await
}

fn decode(
    request: Result<Json<NodeControlRequest>, JsonRejection>,
) -> Result<NodeControlRequest, ProtocolError> {
    request
        .map(|Json(request)| request)
        .map_err(|error| ProtocolError {
            code: ErrorCode::GraphInvalid,
            parameters: serde_json::json!({
                "reason": error.body_text(), "nodeControlOutcome": "not_applied"
            }),
        })
}

#[cfg(test)]
mod tests;
