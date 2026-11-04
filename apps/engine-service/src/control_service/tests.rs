use super::*;
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

use axum::{
    middleware::{self, Next},
    routing::post,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Notify,
};

#[tokio::test]
async fn a_missing_socket_directory_is_a_bind_failure() {
    let error = ControlService::bind(Err(io::Error::other("no session")))
        .await
        .unwrap_err();
    assert!(matches!(error, ControlServiceError::Bind { .. }));
    let fixture = fixture();
    assert!(
        ControlService::bind(Ok(fixture.path().to_owned()))
            .await
            .is_ok()
    );
}

/// An isolated socket directory; never the session's real one.
fn fixture() -> mixrelay_test_support::FixtureDir {
    mixrelay_test_support::FixtureDir::ipc("eng").expect("fixture")
}

fn backend() -> Arc<dyn ControlBackend> {
    Arc::new(crate::EngineGraphController::new_with_configuration(
        Arc::new(mixrelay_render::ProcessorExchange::new(1)),
        crate::EngineGraphControllerConfig {
            clock: Arc::new(mixrelay_time::ManualClock::default()),
            format: mixrelay_audio_graph::RenderFormat {
                sample_rate: 48_000,
                block_frames: 128,
            },
            meters: mixrelay_engine_core::default_peak_meter_bank(),
            initial_bindings: vec![],
            endpoint_backend: Arc::new(
                mixrelay_engine_core::ContractOnlyVirtualEndpointBackend::default(),
            ),
            required_endpoint_node_id: Some(mixrelay_engine_core::DEFAULT_ENDPOINT_NODE_ID.into()),
            processor_catalog: mixrelay_processor_host::ProcessorCatalog::default(),
            graph_resources: Arc::new(mixrelay_engine_core::NoopGraphResourceBackend),
            runtime_status: Arc::new(mixrelay_engine_core::FixedRuntimeStatus::new(
                mixrelay_control_protocol::DriverState::Ready,
            )),
        },
    ))
}

struct BlockingPowerBackend {
    started: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

fn unavailable_control<T>() -> Result<T, mixrelay_control_protocol::ProtocolError> {
    Err(mixrelay_control_protocol::ProtocolError {
        code: mixrelay_control_protocol::ErrorCode::ControlUnavailable,
        parameters: serde_json::json!({}),
    })
}

impl ControlBackend for BlockingPowerBackend {
    fn status(&self) -> mixrelay_control_protocol::AppStatus {
        mixrelay_control_protocol::AppStatus::default()
    }

    fn graph(&self) -> mixrelay_audio_graph::GraphDocument {
        mixrelay_audio_graph::GraphDocument::new(0, vec![], vec![])
    }

    fn graph_status(&self) -> mixrelay_control_protocol::GraphRuntimeStatus {
        mixrelay_control_protocol::GraphRuntimeStatus {
            pending_reason: None,
            protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
            active_revision: 0,
            submitted_revision: 0,
        }
    }

    fn meters(&self) -> mixrelay_control_protocol::MeterSnapshot {
        mixrelay_control_protocol::MeterSnapshot {
            protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
            revision: 0,
            nodes: vec![],
        }
    }

    fn processors(&self) -> mixrelay_control_protocol::ExternalProcessorCatalog {
        mixrelay_control_protocol::ExternalProcessorCatalog {
            protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
            processors: vec![],
            diagnostics: vec![],
        }
    }

    fn set_engine_power(
        &self,
        _request: mixrelay_control_protocol::EnginePowerRequest,
    ) -> Result<
        mixrelay_control_protocol::GraphApplyResponse,
        mixrelay_control_protocol::ProtocolError,
    > {
        self.started.store(true, Ordering::Release);
        while !self.released.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(1));
        }
        Ok(mixrelay_control_protocol::GraphApplyResponse {
            protocol_version: mixrelay_control_protocol::PROTOCOL_VERSION,
            revision: 1,
            state: mixrelay_control_protocol::GraphApplyState::Active,
            persistence_error: None,
        })
    }

    fn rescan_processors(
        &self,
        _request: mixrelay_control_protocol::ProcessorScanRequest,
    ) -> Result<
        mixrelay_control_protocol::ExternalProcessorCatalog,
        mixrelay_control_protocol::ProtocolError,
    > {
        unavailable_control()
    }

    fn set_processor_parameter(
        &self,
        _node_id: &str,
        _parameter_id: u32,
        _request: mixrelay_control_protocol::ProcessorParameterUpdate,
    ) -> Result<
        mixrelay_control_protocol::ProcessorParameterUpdateResponse,
        mixrelay_control_protocol::ProtocolError,
    > {
        unavailable_control()
    }

    fn processor_editor_status(
        &self,
        _node_id: &str,
    ) -> Result<
        mixrelay_control_protocol::ProcessorEditorStatus,
        mixrelay_control_protocol::ProtocolError,
    > {
        unavailable_control()
    }

    fn set_processor_editor(
        &self,
        _node_id: &str,
        _request: mixrelay_control_protocol::ProcessorEditorRequest,
    ) -> Result<
        mixrelay_control_protocol::ProcessorEditorStatus,
        mixrelay_control_protocol::ProtocolError,
    > {
        unavailable_control()
    }

    fn apply_graph_request(
        &self,
        _request: mixrelay_control_protocol::GraphApplyRequest,
    ) -> Result<
        mixrelay_control_protocol::GraphApplyResponse,
        mixrelay_control_protocol::ProtocolError,
    > {
        Err(mixrelay_control_protocol::ProtocolError {
            code: mixrelay_control_protocol::ErrorCode::ControlUnavailable,
            parameters: serde_json::json!({}),
        })
    }
}

async fn request(address: &std::path::Path, bytes: &[u8]) -> Vec<u8> {
    let mut stream = UnixStream::connect(address).await.expect("socket client");
    stream.write_all(bytes).await.expect("request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("bounded response")
        .expect("read response");
    response
}

#[tokio::test]
async fn occupied_control_address_rejects_admission_and_leaves_listener_alive() {
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("first owner");
    let address = service.listener.path().to_path_buf();

    let error = ControlService::bind_in(address.parent().unwrap())
        .await
        .unwrap_err();

    assert!(matches!(&error, ControlServiceError::Bind { source, .. }
        if source.kind() == io::ErrorKind::AddrInUse));
    assert!(error.to_string().contains(&address.display().to_string()));
    let connection = UnixStream::connect(&address)
        .await
        .expect("original listener");
    let mut service = service;
    let _accepted = axum::serve::Listener::accept(&mut service.listener).await;
    drop(connection);
}

#[tokio::test]
async fn dropping_admission_after_startup_failure_releases_the_address() {
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let failed = async move {
        let _service = service;
        Err::<(), _>("native preparation failed")
    }
    .await;

    assert!(failed.is_err());
    let replacement = ControlService::bind_in(address.parent().unwrap())
        .await
        .expect("rebind");
    assert_eq!(replacement.listener.path(), address);
}

#[tokio::test]
async fn public_service_keeps_existing_routes_and_stops_observation_once() {
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let stopped = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&stopped);
    let (stop, shutdown) = oneshot::channel();
    let server = tokio::spawn(service.serve(
        backend(),
        async {
            shutdown.await.expect("shutdown request");
            Ok(())
        },
        move || {
            observed.fetch_add(1, Ordering::SeqCst);
        },
    ));

    let response = request(
        &address,
        b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert_eq!(stopped.load(Ordering::SeqCst), 0);
    stop.send(()).expect("signal shutdown");
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("bounded shutdown")
        .expect("server task")
        .expect("serve");
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    ControlService::bind_in(address.parent().unwrap())
        .await
        .expect("released endpoint");
}

#[tokio::test]
async fn accepted_transaction_finishes_after_observation_stops() {
    let entered = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let completed = Arc::new(AtomicUsize::new(0));
    let (started, release, count) = (
        Arc::clone(&entered),
        Arc::clone(&finish),
        Arc::clone(&completed),
    );
    let router = Router::new().route(
        "/transaction",
        post(move || async move {
            started.notify_one();
            release.notified().await;
            count.fetch_add(1, Ordering::SeqCst);
            "committed"
        }),
    );
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let (stop, shutdown) = oneshot::channel();
    let (closed, observing_stopped) = oneshot::channel();
    let server = tokio::spawn(service.serve_router(
        router,
        ControlOperations::new(Box::new(mixrelay_time::ManualClock::new(
            mixrelay_time::Instant::from_origin(Duration::ZERO),
        ))),
        async {
            shutdown.await.expect("shutdown request");
            Ok(())
        },
        move || {
            closed.send(()).expect("close observation");
        },
    ));
    let client = tokio::spawn(async move {
        request(&address, b"POST /transaction HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("accepted");

    stop.send(()).expect("shutdown");
    tokio::time::timeout(Duration::from_secs(2), observing_stopped)
        .await
        .expect("observation stops before drain")
        .expect("observation signal");
    assert!(
        !server.is_finished(),
        "accepted transaction must not be cancelled"
    );
    assert_eq!(completed.load(Ordering::SeqCst), 0);
    finish.notify_one();
    let response = client.await.expect("client");
    assert!(response.ends_with(b"committed"));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("drained")
        .expect("server task")
        .expect("serve");
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn incomplete_ingress_cannot_hold_any_platform_service_open() {
    let entered = Arc::new(Notify::new());
    let observed = Arc::clone(&entered);
    // A short body deadline keeps the drain fast; the product default is two seconds.
    let router = mixrelay_control_server::router(
        ControlOperations::new(Box::new(mixrelay_time::ManualClock::new(
            mixrelay_time::Instant::from_origin(Duration::ZERO),
        )))
        .with_timings(mixrelay_control_server::OperationTimings {
            body_deadline: Duration::from_millis(50),
            ..mixrelay_control_server::OperationTimings::default()
        }),
    )
    .layer(middleware::from_fn(move |request, next: Next| {
        let observed = Arc::clone(&observed);
        async move {
            observed.notify_one();
            next.run(request).await
        }
    }));
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let (stop, shutdown) = oneshot::channel();
    let server = tokio::spawn(service.serve_router(
        router,
        ControlOperations::new(Box::new(mixrelay_time::ManualClock::new(
            mixrelay_time::Instant::from_origin(Duration::ZERO),
        ))),
        async {
            shutdown.await.expect("shutdown request");
            Ok(())
        },
        || {},
    ));
    let mut client = UnixStream::connect(&address).await.expect("client");
    client.write_all(b"PUT /api/v1/audio/power HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{")
        .await.expect("partial body");
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("entered ingress");

    stop.send(()).expect("shutdown");
    tokio::time::timeout(Duration::from_secs(4), server)
        .await
        .expect("incomplete body cannot hold shutdown")
        .expect("server task")
        .expect("serve");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
        .await
        .expect("connection closed")
        .expect("response");
    assert!(response.starts_with(b"HTTP/1.1 408"));
}

#[tokio::test]
async fn signal_failure_still_stops_observation_and_releases_the_endpoint() {
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let stopped = AtomicUsize::new(0);

    let result = service
        .serve(
            backend(),
            async { Err(io::Error::other("signal registration failed")) },
            || {
                stopped.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;

    assert!(matches!(result, Err(ControlServiceError::Shutdown(_))));
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    ControlService::bind_in(address.parent().unwrap())
        .await
        .expect("released endpoint");
}

#[tokio::test]
async fn disconnected_operation_receipt_drains_before_control_service_returns() {
    let started = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicBool::new(false));
    let fixture = fixture();
    let service = ControlService::bind_in(fixture.path())
        .await
        .expect("reservation");
    let address = service.listener.path().to_path_buf();
    let (stop, shutdown) = oneshot::channel();
    let backend = Arc::new(BlockingPowerBackend {
        started: started.clone(),
        released: released.clone(),
    });
    let server = tokio::spawn(service.serve(
        backend,
        async {
            shutdown.await.expect("shutdown request");
            Ok(())
        },
        || {},
    ));

    let mut client = UnixStream::connect(&address).await.expect("client");
    let body = br#"{"enabled":true,"expectedRevision":null}"#;
    let request = format!(
        "PUT /api/v1/audio/power HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        std::str::from_utf8(body).expect("request body")
    );
    client.write_all(request.as_bytes()).await.expect("request");
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("backend started");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("pending response")
        .expect("response read");
    assert!(response.starts_with(b"HTTP/1.1 202"));

    stop.send(()).expect("shutdown");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !server.is_finished(),
        "accepted operation must still be draining"
    );
    released.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("drained service")
        .expect("server task")
        .expect("serve");
}
