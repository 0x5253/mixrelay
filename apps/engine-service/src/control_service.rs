//! Owns local-socket admission and HTTP draining, independently of native audio adapters.
//! The caller binds before preparing devices and retains audio owners until serve finishes.

use std::{
    future::Future,
    future::IntoFuture,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::Router;
use mixrelay_control_server::{
    ControlBackend, ControlOperations, SocketLimits, bounded_listener, router_with_backend,
};
use mixrelay_local_socket::AuthorizedListener;
use thiserror::Error;
use tokio::sync::oneshot;

#[cfg(test)]
mod tests;

/// Exclusive control socket reservation, held before native audio startup. Only
/// processes of the engine's own user may connect (the runtime and editor run as it).
/// Dropping this owner after a preparation failure releases and removes the socket.
#[derive(Debug)]
pub struct ControlService {
    listener: AuthorizedListener,
}

/// Admission, serving and shutdown-signal failures remain distinguishable to the caller.
#[derive(Debug, Error)]
pub enum ControlServiceError {
    /// Another listener or an OS socket error prevented startup admission.
    #[error("could not bind local control server at {}: {source}", path.display())]
    Bind {
        /// The socket path that could not be reserved.
        path: PathBuf,
        /// Original operating-system error.
        source: io::Error,
    },
    /// HTTP serving failed after the address was reserved.
    #[error("local control server failed: {0}")]
    Serve(#[source] io::Error),
    /// Shutdown was requested because signal observation failed; draining still completes.
    #[error("engine shutdown signal failed: {0}")]
    Shutdown(#[source] io::Error),
}

impl ControlService {
    /// Reserves the engine socket in `directory` (normally the session's socket directory)
    /// before native publication or device opens.
    ///
    /// # Errors
    /// Returns [`ControlServiceError::Bind`] without acquiring any native audio resource.
    pub async fn bind(directory: io::Result<PathBuf>) -> Result<Self, ControlServiceError> {
        match directory {
            Ok(directory) => Self::bind_in(&directory).await,
            Err(source) => Err(ControlServiceError::Bind {
                path: PathBuf::from(mixrelay_local_socket::ENGINE_SOCKET),
                source,
            }),
        }
    }

    async fn bind_in(directory: &Path) -> Result<Self, ControlServiceError> {
        let uid = mixrelay_local_socket::current_uid();
        mixrelay_local_socket::bind(directory, mixrelay_local_socket::ENGINE_SOCKET, uid)
            .await
            .map(|(listener, socket)| Self {
                listener: AuthorizedListener::new(listener, socket, vec![uid]),
            })
            .map_err(|source| ControlServiceError::Bind {
                path: directory.join(mixrelay_local_socket::ENGINE_SOCKET),
                source,
            })
    }

    /// Serves the existing engine routes until the adapter's shutdown future resolves.
    /// The service closes operation admission before `stop_observing`, then drains accepted
    /// HTTP work before native owners are allowed to finalize.
    /// Native finalization remains with the caller, after this method returns.
    ///
    /// # Errors
    /// Returns a serving failure or a shutdown-observation failure. A failed shutdown
    /// observer still stops admission and drains accepted transactions before returning.
    pub async fn serve(
        self,
        backend: Arc<dyn ControlBackend>,
        shutdown: impl Future<Output = io::Result<()>>,
        stop_observing: impl FnOnce(),
    ) -> Result<(), ControlServiceError> {
        let operations = ControlOperations::new(Box::new(mixrelay_time::StdClock::new()));
        let router = router_with_backend(backend, operations.clone());
        self.serve_router(router, operations, shutdown, stop_observing)
            .await
    }

    async fn serve_router(
        self,
        router: Router,
        operations: ControlOperations,
        shutdown: impl Future<Output = io::Result<()>>,
        stop_observing: impl FnOnce(),
    ) -> Result<(), ControlServiceError> {
        let (stop, stopped) = oneshot::channel::<()>();
        let serving = axum::serve(
            bounded_listener(self.listener, SocketLimits::default()),
            router,
        )
        .with_graceful_shutdown(async move {
            let _ = stopped.await;
        })
        .into_future();
        tokio::pin!(serving);
        let result = tokio::select! {
            result = &mut serving => {
                operations.close_admission();
                stop_observing();
                result.map_err(ControlServiceError::Serve)
            }
            signal = shutdown => {
                operations.close_admission();
                stop_observing();
                let _ = stop.send(());
                match serving.await {
                    Ok(()) => signal.map_err(ControlServiceError::Shutdown),
                    Err(error) => Err(ControlServiceError::Serve(error)),
                }
            }
        };
        operations.drain().await;
        result
    }
}
