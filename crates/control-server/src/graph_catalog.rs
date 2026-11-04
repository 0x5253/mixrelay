//! Saved graphs: the catalog, its documents and their editor layouts.
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::{
    ErrorCode, GraphCatalogRequest, GraphLayoutRequest, ProtocolError,
};

use crate::{
    ControlState,
    reply::{catalog_task, failure, reply, unprocessable},
};

pub(crate) async fn catalog(State(state): State<ControlState>) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.graph_catalog())).await
}

pub(crate) async fn document(
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.graph_catalog_document(&id))).await
}

pub(crate) async fn mutate(
    State(state): State<ControlState>,
    request: Result<Json<GraphCatalogRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => return unprocessable(ErrorCode::GraphInvalid, &rejection),
    };
    catalog_task(move || match state.backend.mutate_graph_catalog(request) {
        Ok(response) => Json(response).into_response(),
        Err(mut error) => {
            error.parameters["catalogOutcome"] = serde_json::json!("not_applied");
            failure(error)
        }
    })
    .await
}

pub(crate) async fn layout(State(state): State<ControlState>, Path(id): Path<String>) -> Response {
    catalog_task(move || reply(StatusCode::OK, state.backend.graph_layout(&id))).await
}

pub(crate) async fn update_layout(
    State(state): State<ControlState>,
    Path(id): Path<String>,
    request: Result<Json<GraphLayoutRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ProtocolError {
                    code: ErrorCode::GraphInvalid,
                    parameters: serde_json::json!({
                        "resource": "graph_layout",
                        "graphId": id,
                        "reason": rejection.body_text(),
                    }),
                }),
            )
                .into_response();
        }
    };
    catalog_task(move || {
        reply(
            StatusCode::OK,
            state.backend.update_graph_layout(&id, request),
        )
    })
    .await
}

#[cfg(test)]
mod tests;
