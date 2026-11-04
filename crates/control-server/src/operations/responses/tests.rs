use axum::{body::Body, extract::Request, http::StatusCode};
use serde_json::json;

use super::admission_full_response;
use crate::operations::test_support::json_body;

#[tokio::test]
async fn read_admission_failure_has_no_mutation_outcome() {
    let response = admission_full_response(
        &Request::get(crate::GRAPH_CATALOG_ROUTE)
            .body(Body::empty())
            .expect("read request"),
    );
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(response).await["parameters"], json!({}));
}

#[tokio::test]
async fn admission_failure_uses_the_target_transaction_outcome() {
    for (path, outcome) in [
        (crate::GRAPH_CATALOG_ROUTE, "catalogOutcome"),
        ("/api/v1/workspace", "workspaceOutcome"),
        ("/api/v1/audio/power", "powerOutcome"),
        ("/api/v1/graphs/a/nodes/b/control", "nodeControlOutcome"),
        (crate::GRAPH_ROUTE, "operationOutcome"),
    ] {
        let request = Request::put(path).body(Body::empty()).expect("request");
        let response = admission_full_response(&request);
        assert_eq!(
            json_body(response).await["parameters"],
            json!({outcome: "not_applied"}),
            "{path}"
        );
    }
}
