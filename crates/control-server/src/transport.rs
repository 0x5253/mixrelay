use std::{
    future::Future,
    io::{self, IoSlice},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use axum::serve::Listener;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Sleep,
};

const WRITE_STALL_LIMIT: Duration = Duration::from_secs(2);
const MAX_ACCEPTED_CONTROL_SOCKETS: usize = 64;

#[cfg(test)]
mod tests;

/// Wraps the bound control listener (the engine's authenticated Unix socket, or a test
/// listener) with a 64-socket acceptance limit and a two-second socket write-stall limit.
///
/// The listener retains ownership of each accepted socket. Only a pending socket write,
/// flush or shutdown starts a deadline; waiting for a mutation to produce its response
/// does not. Read/body deadlines remain the HTTP ingress layer's responsibility.
#[must_use]
pub fn bounded_listener<L: Listener>(listener: L) -> impl Listener<Addr = L::Addr> {
    ControlListener::with_capacity(listener, MAX_ACCEPTED_CONTROL_SOCKETS)
}

struct ControlListener<L> {
    listener: L,
    accepted_slots: Arc<Semaphore>,
}

impl<L: Listener> ControlListener<L> {
    fn with_capacity(listener: L, capacity: usize) -> Self {
        Self {
            listener,
            accepted_slots: Arc::new(Semaphore::new(capacity)),
        }
    }
}

impl<L: Listener> Listener for ControlListener<L> {
    type Io = ControlSocket<L::Io>;
    type Addr = L::Addr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let Ok(permit) = Arc::clone(&self.accepted_slots).acquire_owned().await else {
            // Only `Semaphore::close` makes acquisition fail, and this private semaphore is
            // never closed, so a failure here is a programming error in this listener.
            unreachable!("control socket semaphore was closed during the listener lifetime");
        };
        let (stream, address) = Listener::accept(&mut self.listener).await;
        (
            ControlSocket {
                stream,
                _accepted_slot: permit,
                stalled: None,
            },
            address,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

struct ControlSocket<S> {
    stream: S,
    _accepted_slot: OwnedSemaphorePermit,
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<S> ControlSocket<S> {
    fn bound_write<T>(
        &mut self,
        result: Poll<io::Result<T>>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<T>> {
        if result.is_ready() {
            self.stalled = None;
            return result;
        }
        let deadline = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(WRITE_STALL_LIMIT)));
        if deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control response write stalled",
            )));
        }
        Poll::Pending
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for ControlSocket<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ControlSocket<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write(cx, buffer);
        self.bound_write(result, cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.stream).poll_write_vectored(cx, buffers);
        self.bound_write(result, cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.stream).poll_flush(cx);
        self.bound_write(result, cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.stream).poll_shutdown(cx);
        self.bound_write(result, cx)
    }
}
