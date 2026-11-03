//! Control-plane ownership of the XPC listener and periodic publication timer.
mod native;
#[cfg(test)]
mod tests;

/// Startup does not publish a partially constructed dispatch owner.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum StartError {
    #[error("could not create the driver callback queue")]
    Queue,
    #[error("could not create the driver XPC listener")]
    Listener,
    #[error("could not create the driver control timer")]
    Timer,
}

/// The backend owns only native calls; construction and cleanup stay in Rust.
pub(super) trait Backend {
    type Listener;
    type Timer;

    fn listener(&self) -> Option<Self::Listener>;
    fn timer(&self) -> Option<Self::Timer>;
    fn activate(&self, listener: &Self::Listener, timer: &Self::Timer);
    fn discard_listener(&self, listener: &Self::Listener);
    fn cancel_listener(&self, listener: &Self::Listener);
    fn cancel_timer(&self, timer: &Self::Timer);
    fn release_listener(&self, listener: &Self::Listener);
    fn release_timer(&self, timer: &Self::Timer);
}

/// Unique listener/timer ownership, serialized by the service control mutex.
pub(super) struct Dispatch<B: Backend = native::Native> {
    backend: B,
    listener: B::Listener,
    timer: B::Timer,
}

impl Dispatch {
    pub(super) fn start(service: &std::sync::Arc<crate::Service>) -> Result<Self, StartError> {
        let native =
            native::Native::new(std::sync::Arc::downgrade(service)).ok_or(StartError::Queue)?;
        Self::with_backend(native)
    }
}

impl<B: Backend> Dispatch<B> {
    fn with_backend(backend: B) -> Result<Self, StartError> {
        let listener = backend.listener().ok_or(StartError::Listener)?;
        let Some(timer) = backend.timer() else {
            // XPC forbids releasing an inactive connection. Cancellation plus
            // activation with an inert handler makes this failed start releasable.
            backend.discard_listener(&listener);
            return Err(StartError::Timer);
        };
        backend.activate(&listener, &timer);
        Ok(Self {
            backend,
            listener,
            timer,
        })
    }
}

impl<B: Backend> Drop for Dispatch<B> {
    fn drop(&mut self) {
        // The callback queue retains a weak service context until all native callbacks
        // drain. Cancellation never exposes this Dispatch storage to a callback.
        self.backend.cancel_listener(&self.listener);
        self.backend.cancel_timer(&self.timer);
        self.backend.release_listener(&self.listener);
        self.backend.release_timer(&self.timer);
    }
}
