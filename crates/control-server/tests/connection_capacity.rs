#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a failed setup fails the test"
)]
//! Opt-in, same-workload socket ownership measurement; not an audio benchmark.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, routing::get, serve::Listener};
use mixrelay_control_server::bounded_listener;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time,
};

async fn acknowledged_keep_alive(address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut reader = BufReader::new(stream);
    time::timeout(Duration::from_secs(2), async {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert_eq!(line, "HTTP/1.1 200 OK\r\n");
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).await.unwrap() > 0);
            if line == "\r\n" {
                break;
            }
        }
        let mut body = [0; 7];
        reader.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"healthy");
    })
    .await
    .expect("server must acknowledge each retained connection");
    assert!(reader.buffer().is_empty());
    reader.into_inner()
}

#[tokio::test]
async fn saturated_listener_resumes_http_service_after_one_client_disconnects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            bounded_listener(listener),
            Router::new().route("/health", get(|| async { "healthy" })),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let mut idle = Vec::with_capacity(64);
    for _ in 0..64 {
        idle.push(acknowledged_keep_alive(address).await);
    }
    let mut health = TcpStream::connect(address).await.unwrap();
    health
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut first_byte = [0];
    assert!(
        time::timeout(Duration::from_millis(100), health.read(&mut first_byte))
            .await
            .is_err()
    );
    drop(idle.pop());
    let mut response = String::new();
    time::timeout(Duration::from_secs(2), health.read_to_string(&mut response))
        .await
        .expect("capacity must resume after the socket owner is dropped")
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("healthy"));
    drop(idle);
    drop(health);
    stop.send(()).unwrap();
    time::timeout(Duration::from_secs(2), server)
        .await
        .expect("all connection owners must drain on shutdown")
        .unwrap();
}

#[tokio::test]
async fn saturated_idle_connections_do_not_hold_graceful_shutdown_open() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            bounded_listener(listener),
            Router::new().route("/health", get(|| async { "healthy" })),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
        .unwrap();
    });
    let mut idle = Vec::with_capacity(65);
    for _ in 0..64 {
        idle.push(acknowledged_keep_alive(address).await);
    }
    idle.push(TcpStream::connect(address).await.unwrap());
    stop.send(()).unwrap();
    time::timeout(Duration::from_secs(2), server)
        .await
        .expect("server shutdown must cancel pending admission and close idle owners")
        .unwrap();
    drop(idle);
}

#[tokio::test]
#[ignore = "explicit before/after socket ownership measurement"]
async fn retained_connections_under_identical_128_client_load() {
    let expected: usize = std::env::var("MIXRELAY_EXPECTED_ACCEPTED")
        .expect("set the expected admission count for this source revision")
        .parse()
        .expect("the expected count must be numeric");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut listener = bounded_listener(listener);
    let accepted = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&accepted);
    let (stop, mut stopped) = oneshot::channel();
    let owner = tokio::spawn(async move {
        let mut sockets = Vec::new();
        loop {
            tokio::select! {
                () = async { let _ = (&mut stopped).await; } => break,
                (socket, _) = listener.accept() => {
                    sockets.push(socket);
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        sockets.len()
    });
    let mut clients = Vec::with_capacity(128);
    for _ in 0..128 {
        clients.push(
            time::timeout(Duration::from_secs(2), TcpStream::connect(address))
                .await
                .expect("local client connect deadline")
                .unwrap(),
        );
    }
    time::sleep(Duration::from_millis(250)).await;
    let retained = accepted.load(Ordering::SeqCst);
    stop.send(()).unwrap();
    let released = time::timeout(Duration::from_secs(2), owner)
        .await
        .expect("listener owner shutdown deadline")
        .unwrap();
    drop(clients);
    println!("clients=128 observation_ms=250 retained={retained} released={released}");
    assert_eq!(retained, expected);
    assert_eq!(released, retained);
}
