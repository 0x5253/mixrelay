//! Graph-owned source XPC listener with explicit control-plane lifetime.
mod native;
#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum StartError {
    #[error("could not create the engine source XPC listener")]
    Listener,
}

pub(super) trait Backend {
    type Listener;
    fn listener(&self) -> Option<Self::Listener>;
    fn activate(&self, listener: &Self::Listener);
    fn cancel_listener(&self, listener: &Self::Listener);
    fn release_listener(&self, listener: &Self::Listener);
}

pub(super) struct Dispatch<B: Backend = native::Native> {
    backend: B,
    listener: B::Listener,
}

impl Dispatch {
    pub(super) fn start() -> Result<Self, StartError> {
        Self::with_backend(native::Native)
    }
}

impl<B: Backend> Dispatch<B> {
    fn with_backend(backend: B) -> Result<Self, StartError> {
        let listener = backend.listener().ok_or(StartError::Listener)?;
        backend.activate(&listener);
        Ok(Self { backend, listener })
    }
}

impl<B: Backend> Drop for Dispatch<B> {
    fn drop(&mut self) {
        // Static callbacks retain SERVICE; they never borrow this dispatch owner.
        self.backend.cancel_listener(&self.listener);
        self.backend.release_listener(&self.listener);
    }
}
