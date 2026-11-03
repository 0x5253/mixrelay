use super::ControlListener;
use axum::serve::Listener;
use std::{
    io,
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, net::TcpListener, time::timeout};

async fn test_listener(capacity: usize) -> (ControlListener<TcpListener>, SocketAddr) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("test listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have an address");
    (ControlListener::with_capacity(listener, capacity), address)
}

async fn connect(address: SocketAddr) -> tokio::net::TcpStream {
    tokio::net::TcpStream::connect(address)
        .await
        .expect("test client should connect")
}

#[tokio::test]
async fn accepted_socket_count_stays_within_configured_capacity() {
    let (mut listener, address) = test_listener(2).await;
    let mut clients = Vec::new();
    let mut sockets = Vec::new();

    for _ in 0..2 {
        let client = connect(address).await;
        let (socket, _) = listener.accept().await;
        clients.push(client);
        sockets.push(socket);
    }

    assert_eq!(listener.accepted_slots.available_permits(), 0);
    let third_client = connect(address).await;
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );

    drop(
        sockets
            .pop()
            .expect("two sockets should have been accepted"),
    );
    let (third_socket, _) = timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect("dropping one socket should release one slot");
    drop(third_socket);
    drop(third_client);
    drop(clients);
    drop(sockets);
}

#[tokio::test]
async fn dropping_socket_releases_one_slot_for_the_next_accept() {
    let (mut listener, address) = test_listener(1).await;
    let first_client = connect(address).await;
    let (first_socket, _) = listener.accept().await;
    let second_client = connect(address).await;

    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    drop(first_socket);

    let (second_socket, _) = timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect("dropping the first socket should release its slot");
    drop(second_socket);
    drop(second_client);
    drop(first_client);
}

#[tokio::test]
async fn cancelling_accept_after_reservation_releases_the_slot() {
    let (mut listener, address) = test_listener(1).await;

    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(listener.accepted_slots.available_permits(), 1);

    let client = connect(address).await;
    let (socket, _) = timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect("a cancelled accept should not leak its reservation");
    drop(socket);
    drop(client);
}

#[tokio::test]
async fn dropping_listener_does_not_release_a_live_socket_slot() {
    let (mut listener, address) = test_listener(1).await;
    let slots = Arc::clone(&listener.accepted_slots);
    let client = connect(address).await;
    let (socket, _) = listener.accept().await;

    assert_eq!(slots.available_permits(), 0);
    drop(listener);
    assert_eq!(slots.available_permits(), 0);

    drop(socket);
    assert_eq!(slots.available_permits(), 1);
    drop(client);
}

#[tokio::test]
async fn half_closing_a_socket_does_not_release_its_admission_slot() {
    let (mut listener, address) = test_listener(1).await;
    let client = connect(address).await;
    let (mut socket, _) = listener.accept().await;
    socket.shutdown().await.unwrap();
    assert_eq!(listener.accepted_slots.available_permits(), 0);
    drop(socket);
    assert_eq!(listener.accepted_slots.available_permits(), 1);
    drop(client);
}

#[tokio::test]
async fn ready_write_clears_an_existing_stall_deadline() {
    let (mut listener, address) = test_listener(1).await;
    let client = connect(address).await;
    let (mut socket, _) = listener.accept().await;
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    assert!(
        socket
            .bound_write(Poll::<io::Result<usize>>::Pending, &mut context)
            .is_pending()
    );
    assert!(socket.stalled.is_some());

    let result = socket.bound_write(Poll::Ready(Ok(1)), &mut context);
    assert!(matches!(result, Poll::Ready(Ok(1))));
    assert!(socket.stalled.is_none());
    drop(client);
}

#[tokio::test]
async fn expired_stall_deadline_returns_a_timeout_error() {
    let (mut listener, address) = test_listener(1).await;
    let client = connect(address).await;
    let (mut socket, _) = listener.accept().await;
    let mut deadline = Box::pin(tokio::time::sleep(Duration::ZERO));
    deadline.as_mut().await;
    socket.stalled = Some(deadline);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    let result = socket.bound_write(Poll::<io::Result<usize>>::Pending, &mut context);
    assert!(matches!(
        result,
        Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::TimedOut
    ));
    drop(client);
}
