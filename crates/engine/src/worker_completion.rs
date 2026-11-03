//! Audio-worker completion closes control admission and preserves its original failure.
use std::{error::Error as StdError, future::Future, io, sync::Arc};
use tokio::sync::oneshot;

#[derive(Debug, thiserror::Error)]
pub enum Error<E> {
    #[error("audio worker failed: {0}")]
    Audio(#[source] Arc<E>),
    #[error("audio worker exited before shutdown was requested")]
    UnexpectedExit,
    #[error("audio worker completion channel closed without a result")]
    MissingResult,
}

impl<E> Clone for Error<E> {
    fn clone(&self) -> Self {
        match self {
            Self::Audio(error) => Self::Audio(Arc::clone(error)),
            Self::UnexpectedExit => Self::UnexpectedExit,
            Self::MissingResult => Self::MissingResult,
        }
    }
}

pub struct Completion<E> {
    receiver: oneshot::Receiver<Result<(), E>>,
    observed: Option<Result<(), Error<E>>>,
}

#[must_use]
pub fn channel<E>() -> (oneshot::Sender<Result<(), E>>, Completion<E>) {
    let (sender, receiver) = oneshot::channel();
    (
        sender,
        Completion {
            receiver,
            observed: None,
        },
    )
}

impl<E: StdError + Send + Sync + 'static> Completion<E> {
    async fn observe(&mut self) {
        if self.observed.is_none() {
            self.observed = Some(match (&mut self.receiver).await {
                Ok(result) => result.map_err(|error| Error::Audio(Arc::new(error))),
                Err(_) => Err(Error::MissingResult),
            });
        }
    }

    /// # Errors
    /// Reports shutdown observation failure or an audio worker that ended first.
    pub async fn shutdown(
        &mut self,
        requested: impl Future<Output = io::Result<()>>,
    ) -> io::Result<()> {
        tokio::select! {
            result = requested => result,
            () = self.observe() => {
                let error = match &self.observed {
                    Some(Err(error)) => error.clone(),
                    Some(Ok(())) => Error::UnexpectedExit,
                    None => Error::MissingResult,
                };
                Err(io::Error::other(error))
            }
        }
    }

    /// Call after requesting worker stop and joining, including observer failure.
    /// # Errors
    /// Reports the retained worker failure or a missing completion result.
    pub async fn finish(mut self) -> Result<(), Error<E>> {
        self.observe().await;
        self.observed.unwrap_or(Err(Error::MissingResult))
    }
}

#[cfg(test)]
mod tests;
