use axum::http::StatusCode;
use mixrelay_control_protocol::{ErrorCode, ProtocolError};
use serde_json::json;

use crate::{
    ControlOperations, router_with_backend,
    test_support::{TestBackend, apply_request, clock, document},
};

#[tokio::test]
async fn typed_graph_failures_map_to_stable_http_statuses() {
    let cases = [
        (ErrorCode::EngineReplaced, StatusCode::CONFLICT),
        (ErrorCode::InvalidOperationId, StatusCode::BAD_REQUEST),
        (ErrorCode::GraphRevisionStale, StatusCode::CONFLICT),
        (ErrorCode::GraphExchangeBusy, StatusCode::CONFLICT),
        (ErrorCode::GraphInvalid, StatusCode::UNPROCESSABLE_ENTITY),
        (
            ErrorCode::GraphBindingUnsupported,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ErrorCode::Unauthorized, StatusCode::UNAUTHORIZED),
        (ErrorCode::PairingRequired, StatusCode::UNAUTHORIZED),
        (ErrorCode::DriverNotLoaded, StatusCode::SERVICE_UNAVAILABLE),
    ];
    for (code, expected_status) in cases {
        let backend = TestBackend::with_result(Err(ProtocolError {
            code,
            parameters: json!({ "revision": 5 }),
        }));
        let (status, response) = apply_request(
            router_with_backend(backend, ControlOperations::new(clock())),
            &document(),
        )
        .await;
        assert_eq!(status, expected_status);
        assert_eq!(response["parameters"]["revision"], 5);
    }
}
