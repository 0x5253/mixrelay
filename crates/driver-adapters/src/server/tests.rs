use axum::http::{Method, header::CONTENT_TYPE};
use axum::{body::Body, http::Request};
use tower::ServiceExt as _;

use super::*;

const RECHECK_ONLY: InventoryIntervals = InventoryIntervals {
    recheck: Duration::from_millis(100),
    after_failure: Duration::from_millis(100),
};
use mixrelay_driver_core::{
    SettingsStore, test_backend::FakeBackend, test_settings::MemorySettings,
};

fn app(store: &MemorySettings) -> Router {
    let (backend, _) = FakeBackend::ready();
    router(
        DriverManager::open(
            backend,
            store.clone(),
            mixrelay_driver_core::test_runtime::runtime(),
        )
        .unwrap(),
        InventoryIntervals::default(),
    )
}

#[tokio::test]
async fn snapshot_uses_independent_native_state_and_version() {
    let store = MemorySettings::default();
    let (backend, _) = FakeBackend::ready();
    let mut manager = DriverManager::open(
        backend,
        store.clone(),
        mixrelay_driver_core::test_runtime::runtime(),
    )
    .unwrap();
    manager.set_installed_version(Some("0.1.0".into()));
    let response = router(manager, InventoryIntervals::default())
        .oneshot(
            Request::builder()
                .uri(SNAPSHOT_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), JSON_LIMIT).await.unwrap();
    let snapshot: DriverSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot.state, mixrelay_driver_core::DriverState::Ready);
    assert_eq!(snapshot.protocol_version, 1);
    assert_eq!(snapshot.installed_version.as_deref(), Some("0.1.0"));
    assert_eq!(snapshot.owner_process_id, Some(42));
}

#[tokio::test]
async fn maintenance_initializes_later_ready_inventory_without_requests() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    observed.lock().state = mixrelay_driver_core::DriverState::InstalledNotLoaded;
    let manager = Arc::new(Mutex::new(
        DriverManager::open(
            backend,
            store.clone(),
            mixrelay_driver_core::test_runtime::runtime(),
        )
        .unwrap(),
    ));
    let maintenance = tokio::spawn(maintain_inventory(Arc::downgrade(&manager), RECHECK_ONLY));
    assert!(store.load().unwrap().is_none());
    observed.lock().state = mixrelay_driver_core::DriverState::Ready;
    tokio::time::timeout(Duration::from_secs(3), async {
        while observed.lock().applies == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("initialization completed without a request");
    assert_eq!(observed.lock().applies, 1);
    assert_eq!(
        manager.lock().snapshot().unwrap().state,
        mixrelay_driver_core::DriverState::Ready
    );
    assert!(store.load().unwrap().is_some());
    drop(manager);
    tokio::time::timeout(Duration::from_secs(3), maintenance)
        .await
        .expect("maintenance released its owner")
        .unwrap();
}

#[tokio::test]
async fn unavailable_router_reports_native_state_and_releases_its_owner_on_drop() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    observed.lock().state = mixrelay_driver_core::DriverState::InstalledNotLoaded;
    let app = router(
        DriverManager::open(
            backend,
            store.clone(),
            mixrelay_driver_core::test_runtime::runtime(),
        )
        .unwrap(),
        InventoryIntervals::default(),
    );
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(SNAPSHOT_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), JSON_LIMIT).await.unwrap();
    let snapshot: DriverSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        snapshot.state,
        mixrelay_driver_core::DriverState::InstalledNotLoaded
    );
    assert!(!snapshot.settings.can_create);
    assert!(store.load().unwrap().is_none());
    drop(app);
    tokio::time::timeout(Duration::from_secs(3), async {
        while Arc::strong_count(&observed) != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("waiting startup task did not retain the router owner");
    observed.lock().state = mixrelay_driver_core::DriverState::Ready;
    assert_eq!(observed.lock().applies, 0);
    assert!(store.load().unwrap().is_none());
}

#[tokio::test]
async fn waiting_startup_task_exits_when_its_router_owner_is_gone() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    observed.lock().state = mixrelay_driver_core::DriverState::InstalledNotLoaded;
    let manager = Arc::new(Mutex::new(
        DriverManager::open(
            backend,
            store.clone(),
            mixrelay_driver_core::test_runtime::runtime(),
        )
        .unwrap(),
    ));
    let owner = Arc::downgrade(&manager);
    let startup = tokio::spawn(maintain_inventory(
        owner.clone(),
        InventoryIntervals::default(),
    ));
    drop(manager);
    tokio::time::timeout(Duration::from_secs(3), startup)
        .await
        .expect("cancelled startup task exited")
        .unwrap();
    assert!(owner.upgrade().is_none());
    assert_eq!(observed.lock().applies, 0);
}

#[tokio::test]
async fn maintenance_recovers_failed_inventory_without_requests_and_releases_router_on_drop() {
    let store = MemorySettings::default();
    let (backend, observed) = FakeBackend::ready();
    let manager = DriverManager::open(
        backend,
        store.clone(),
        mixrelay_driver_core::test_runtime::runtime(),
    )
    .unwrap();
    let saved = store.load().unwrap();
    let readbacks = observed.lock().readbacks;
    let id = observed.lock().settings.devices[0]
        .spec
        .id
        .as_str()
        .to_owned();
    // A short recheck keeps this test fast; it asserts the wait, not its product length.
    let app = router(manager, RECHECK_ONLY);
    {
        let mut native = observed.lock();
        native.state = mixrelay_driver_core::DriverState::Unavailable;
        native.failed_devices.insert(id.clone());
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while observed.lock().readbacks == readbacks {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("failed owner recovered without an API request");
    assert_eq!(observed.lock().recovered_devices, [id]);
    tokio::time::sleep(RECHECK_ONLY.recheck + Duration::from_millis(50)).await;
    assert_eq!(observed.lock().readbacks, readbacks + 1);
    assert_eq!(store.load().unwrap(), saved);
    drop(app);
    tokio::time::timeout(Duration::from_secs(3), async {
        while Arc::strong_count(&observed) != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("maintenance retained a dropped router owner");
    assert_eq!(observed.lock().applies, 2);
}

#[tokio::test]
async fn untrusted_origins_hosts_and_non_json_mutations_are_rejected() {
    let store = MemorySettings::default();
    let app = app(&store);
    for (header, value) in [
        (ORIGIN, "https://example.com"),
        (ORIGIN, "null"),
        (ORIGIN, "http://127.0.0.1:1420"),
        (ORIGIN, "http://localhost:1420"),
        (ORIGIN, "http://tauri.localhost"),
        (ORIGIN, "https://tauri.localhost"),
        (ORIGIN, "tauri://localhost"),
        (ORIGIN, "http://mixrelay-driver"),
        (HOST, "example.com:19342"),
        (HOST, "127.0.0.1:19342"),
        (HOST, "localhost:19342"),
        (HOST, "[::1]:19342"),
        (HOST, "mixrelay-driver:19342"),
    ] {
        let request = Request::builder()
            .method(Method::PUT)
            .uri(SNAPSHOT_ROUTE)
            .header(header, value)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }
    let request = Request::builder()
        .method(Method::PUT)
        .uri(SNAPSHOT_ROUTE)
        .header(CONTENT_TYPE, "text/plain")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn logical_authority_is_allowed_and_body_limit_is_enforced() {
    let store = MemorySettings::default();
    let app = app(&store);
    let request = Request::builder()
        .uri(SNAPSHOT_ROUTE)
        .header(HOST, "mixrelay-driver")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let request = Request::builder()
        .method(Method::PUT)
        .uri(SNAPSHOT_ROUTE)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(vec![b'x'; JSON_LIMIT + 1]))
        .unwrap();
    assert_eq!(
        app.oneshot(request).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn stale_revision_and_missing_identity_are_distinct_failures() {
    let store = MemorySettings::default();
    let app = app(&store);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(SNAPSHOT_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: DriverSnapshot =
        serde_json::from_slice(&to_bytes(response.into_body(), JSON_LIMIT).await.unwrap()).unwrap();
    for (revision, expected_status) in [
        (snapshot.revision - 1, StatusCode::CONFLICT),
        (snapshot.revision, StatusCode::NOT_FOUND),
    ] {
        let body = serde_json::json!({"action":"remove","expectedRevision":revision,"endpointId":"absent"}).to_string();
        let request = Request::builder()
            .method(Method::PUT)
            .uri(SNAPSHOT_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            expected_status
        );
    }
}

#[tokio::test]
async fn inventory_creation_rename_and_removal_are_available_without_credentials() {
    let store = MemorySettings::default();
    let app = app(&store);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(SNAPSHOT_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut snapshot: DriverSnapshot =
        serde_json::from_slice(&to_bytes(response.into_body(), JSON_LIMIT).await.unwrap()).unwrap();
    assert!(snapshot.settings.can_create);
    assert!(snapshot.settings.can_rename);
    let mut spec = snapshot.settings.devices[0].spec.clone();
    spec.id = "second-device".into();
    spec.display_name = "Second device".into();
    for operation in 0..3 {
        let command = match operation {
            0 => DriverCommand::Create {
                expected_revision: snapshot.revision,
                spec: spec.clone(),
            },
            1 => {
                spec.display_name = "Renamed device".into();
                DriverCommand::Configure {
                    expected_revision: snapshot.revision,
                    spec: spec.clone(),
                }
            }
            _ => DriverCommand::Remove {
                expected_revision: snapshot.revision,
                endpoint_id: spec.id.clone(),
            },
        };
        let request = Request::builder()
            .method(Method::PUT)
            .uri(SNAPSHOT_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&command).unwrap()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        snapshot =
            serde_json::from_slice(&to_bytes(response.into_body(), JSON_LIMIT).await.unwrap())
                .unwrap();
        let device = snapshot
            .settings
            .devices
            .iter()
            .find(|device| device.spec.id == spec.id);
        if operation == 2 {
            assert!(device.is_none());
        } else {
            assert_eq!(device.unwrap().spec.display_name, spec.display_name);
        }
        assert_eq!(
            store.load().unwrap().unwrap().devices.len(),
            snapshot.settings.devices.len()
        );
    }
}

#[tokio::test]
async fn runtime_route_claims_and_releases_without_persisting_inventory_revision() {
    let store = MemorySettings::default();
    let (backend, _) = FakeBackend::ready();
    let mut manager = DriverManager::open(
        backend,
        store.clone(),
        mixrelay_driver_core::test_runtime::runtime(),
    )
    .unwrap();
    let revision = manager.snapshot().unwrap().revision;
    let router = router(manager, InventoryIntervals::default());
    let send = |command: mixrelay_driver_core::LatencyCommand| {
        Request::builder()
            .method(Method::PUT)
            .uri(mixrelay_driver_core::LATENCY_ROUTE)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&command).unwrap()))
            .unwrap()
    };
    let response = router
        .clone()
        .oneshot(send(mixrelay_driver_core::LatencyCommand::Claim {
            incarnation: "test-engine".into(),
            process_id: std::process::id(),
        }))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let reply: mixrelay_driver_core::LatencyReply =
        serde_json::from_slice(&to_bytes(response.into_body(), JSON_LIMIT).await.unwrap()).unwrap();
    assert!(reply.accepted);
    let response = router
        .clone()
        .oneshot(send(mixrelay_driver_core::LatencyCommand::Release {
            lease: reply.lease.unwrap(),
        }))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router
        .oneshot(
            Request::builder()
                .uri(SNAPSHOT_ROUTE)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let snapshot: DriverSnapshot =
        serde_json::from_slice(&to_bytes(response.into_body(), JSON_LIMIT).await.unwrap()).unwrap();
    assert_eq!(snapshot.revision, revision);
}
