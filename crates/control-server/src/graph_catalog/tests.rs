use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::CONTENT_TYPE},
};
use mixrelay_control_protocol::{GraphApplyResponse, GraphApplyState, PROTOCOL_VERSION};
use serde_json::json;

use crate::{
    ControlOperations, GRAPH_CATALOG_ROUTE, router_with_backend,
    test_support::{TestBackend, clock, response_json},
};

#[tokio::test]
async fn graph_layout_routes_preserve_ui_identities_and_use_independent_revision() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    let route = "/api/v1/graphs/saved/layout";
    let (status, body) = response_json(
        router_with_backend(backend.clone(), ControlOperations::new(clock())),
        Request::builder().uri(route).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["graphId"], "saved");
    assert_eq!(body["layoutRevision"], 4);
    let layout = json!({"positions":{"duplex:input":{"x":-50.0,"y":75.0}},"viewport":{"x":-100.0,"y":80.0,"zoom":0.5}});
    for (revision, expected_status) in [(4, StatusCode::OK), (3, StatusCode::CONFLICT)] {
        let (status, body) = response_json(
            router_with_backend(backend.clone(), ControlOperations::new(clock())),
            Request::builder()
                .method(Method::PUT)
                .uri(route)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"expectedLayoutRevision":revision,"layout":layout}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, expected_status);
        if status == StatusCode::OK {
            assert_eq!(body["layout"], layout);
            assert_eq!(body["layoutRevision"], 5);
        } else {
            assert_eq!(body["code"], "graph_revision_stale");
            assert_eq!(body["parameters"]["resource"], "graph_layout");
        }
    }
    let invalid =
        json!({"expectedLayoutRevision":4,"layout":{"positions":{"node":{"x":null,"y":1}}}});
    let (status, body) = response_json(
        router_with_backend(backend, ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri(route)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(invalid.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["parameters"]["resource"], "graph_layout");
    assert_eq!(body["parameters"]["graphId"], "saved");
}

#[tokio::test]
async fn catalog_routes_preserve_document_identity_and_both_concurrency_baselines() {
    let backend = TestBackend::with_result(Ok(GraphApplyResponse {
        protocol_version: PROTOCOL_VERSION,
        revision: 5,
        state: GraphApplyState::Active,
        persistence_error: None,
    }));
    for route in [GRAPH_CATALOG_ROUTE, "/api/v1/graphs/saved"] {
        let (status, body) = response_json(
            router_with_backend(backend.clone(), ControlOperations::new(clock())),
            Request::builder().uri(route).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if route == GRAPH_CATALOG_ROUTE {
            assert_eq!(body["activeId"], "saved");
            assert_eq!(body["catalogRevision"], 7);
            assert_eq!(body["document"]["revision"], 5);
        } else {
            assert_eq!(body["revision"], 5);
        }
    }
    for (revision, expected_status) in [(7, StatusCode::OK), (6, StatusCode::CONFLICT)] {
        let (status, body) = response_json(router_with_backend(backend.clone(), ControlOperations::new(clock())), Request::builder().method(Method::PUT).uri(GRAPH_CATALOG_ROUTE).header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"action":"rename","id":"saved","name":"SPAT","expectedRevision":5,"expectedCatalogRevision":revision}).to_string())).unwrap()).await;
        assert_eq!(status, expected_status);
        if status == StatusCode::OK {
            assert_eq!(body["graphs"][0]["name"], "SPAT");
        } else {
            assert_eq!(body["code"], "graph_revision_stale");
            assert_eq!(body["parameters"]["catalogOutcome"], "not_applied");
        }
    }
    let (status, _) = response_json(
        router_with_backend(backend, ControlOperations::new(clock())),
        Request::builder()
            .method(Method::PUT)
            .uri(GRAPH_CATALOG_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
