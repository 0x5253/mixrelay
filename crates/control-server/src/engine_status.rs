//! Engine status and meter reads.
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use mixrelay_control_protocol::MeterSnapshot;

use crate::{ControlState, reply::catalog_task};

pub(crate) async fn status(State(state): State<ControlState>) -> Response {
    catalog_task(move || Json(state.backend.status()).into_response()).await
}

pub(crate) async fn meters(State(state): State<ControlState>) -> Json<MeterSnapshot> {
    Json(state.backend.meters())
}
