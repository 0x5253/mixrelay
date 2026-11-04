use std::{future::Future, io};

use mixrelay_control_server::{ControlOperations, SocketLimits, bounded_listener};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A standalone server without audio, on the engine's user-owned socket; never a network
    // listener.
    let directory = mixrelay_local_socket::socket_directory()?;
    let uid = mixrelay_local_socket::current_uid();
    let (listener, socket) =
        mixrelay_local_socket::bind(&directory, mixrelay_local_socket::ENGINE_SOCKET, uid).await?;
    tracing::info!(path = %socket.path().display(), "MixRelay control server listening");
    let listener = mixrelay_local_socket::AuthorizedListener::new(listener, socket, vec![uid]);
    let operations = ControlOperations::new(Box::new(mixrelay_time::StdClock::new()));
    let result = axum::serve(
        bounded_listener(listener, SocketLimits::default()),
        mixrelay_control_server::router(operations.clone()),
    )
    .with_graceful_shutdown(stop_admission_on_signal(
        operations.clone(),
        tokio::signal::ctrl_c(),
    ))
    .await;
    operations.close_admission();
    operations.drain().await;
    result?;
    Ok(())
}

async fn stop_admission_on_signal(
    operations: ControlOperations,
    signal: impl Future<Output = io::Result<()>>,
) {
    if let Err(error) = signal.await {
        tracing::error!(%error, "control server shutdown signal failed");
    }
    operations.close_admission();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn successful_and_failed_signals_close_admission_before_returning() {
        for result in [Ok(()), Err(io::Error::other("signal observation failed"))] {
            let operations = ControlOperations::new(Box::new(mixrelay_time::ManualClock::new(
                mixrelay_time::Instant::from_origin(std::time::Duration::ZERO),
            )));
            let router = mixrelay_control_server::router(operations.clone());
            let graph_request = || {
                Request::get(mixrelay_control_server::GRAPH_ROUTE)
                    .body(Body::empty())
                    .unwrap()
            };
            let before = router.clone().oneshot(graph_request()).await.unwrap();
            assert_eq!(before.status(), axum::http::StatusCode::OK);
            stop_admission_on_signal(operations, std::future::ready(result)).await;
            let after = router.oneshot(graph_request()).await.unwrap();
            assert_eq!(after.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}
