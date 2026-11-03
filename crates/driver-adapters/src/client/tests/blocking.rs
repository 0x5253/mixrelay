//! Repeated control requests retain their authenticated Unix connection pool.

use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Counted {
    inner: mixrelay_local_socket::AuthorizedListener,
    accepted: Arc<AtomicUsize>,
}

impl axum::serve::Listener for Counted {
    type Io = tokio::net::UnixStream;
    type Addr = mixrelay_local_socket::Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let connection = self.inner.accept().await;
        // The test reads after awaited requests; the count carries no other data.
        self.accepted.fetch_add(1, Ordering::Relaxed);
        connection
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

#[tokio::test]
async fn blocking_requests_and_clones_reuse_one_live_driver_connection() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("drv").unwrap();
    let (backend, _) = mixrelay_driver_core::test_backend::FakeBackend::ready();
    let manager = mixrelay_driver_core::DriverManager::open(
        backend,
        mixrelay_driver_core::test_settings::MemorySettings::default(),
        mixrelay_driver_core::test_runtime::runtime(),
    )
    .unwrap();
    let uid = mixrelay_local_socket::current_uid();
    let (listener, socket) = mixrelay_local_socket::bind(fixture.path(), DRIVER_SOCKET, uid)
        .await
        .unwrap();
    let client = DriverClient::with_socket_path(socket.path()).unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = Counted {
        inner: mixrelay_local_socket::AuthorizedListener::new(listener, socket, vec![uid]),
        accepted: accepted.clone(),
    };
    let service = tokio::spawn(async move {
        axum::serve(counted, crate::router(manager)).await.unwrap();
    });
    let worker = client.clone();
    tokio::task::spawn_blocking(move || {
        let initial = worker.snapshot_blocking().unwrap();
        for _ in 0..8 {
            assert_eq!(worker.clone().snapshot_blocking().unwrap(), initial);
        }
        let stopped = worker
            .mutate_blocking(&DriverCommand::Deactivate {
                expected_revision: initial.revision,
            })
            .unwrap();
        assert!(stopped.settings.devices.is_empty());
        assert_eq!(worker.snapshot_blocking().unwrap(), stopped);
    })
    .await
    .unwrap();
    assert_eq!(accepted.load(Ordering::Relaxed), 1);
    // The last client can be held by an async owner; its cached runtime shuts down.
    drop(client);
    service.abort();
    assert!(service.await.unwrap_err().is_cancelled());
}
