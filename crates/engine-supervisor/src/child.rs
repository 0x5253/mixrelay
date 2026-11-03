//! Observes controller ownership and requests the existing engine drain.
use std::{future::Future, io, io::Read, io::Write, thread};

use tokio::sync::oneshot;

/// Engine-side observation of the controller's private pipe lifetime.
/// Construct only for [`crate::CONTROLLER_ARGUMENT`], never for standalone services
/// or plugin subprocesses. No thread, descriptor or wait enters audio callbacks.
#[derive(Debug)]
pub struct EngineOwner {
    closed: oneshot::Receiver<io::Result<()>>,
}

impl EngineOwner {
    /// Observes the inherited stdin pipe until its controller writer closes.
    ///
    /// The dedicated blocking reader is intentionally outside the Tokio runtime:
    /// an OS-triggered exit must not wait for an uncancellable Tokio stdin task.
    /// On pipe closure it exits immediately; on independent engine exit the OS
    /// reclaims the remaining reader with the process, after audio has drained.
    ///
    /// # Errors
    /// Returns a thread-creation error before native audio resources are acquired.
    pub fn from_stdin() -> io::Result<Self> {
        Self::from_reader(io::stdin())
    }

    fn from_reader(mut reader: impl Read + Send + 'static) -> io::Result<Self> {
        let (sender, closed) = oneshot::channel();
        thread::Builder::new()
            .name("mixrelay-controller-owner".into())
            .spawn(move || {
                let mut byte = [0];
                let result = loop {
                    match reader.read(&mut byte) {
                        Ok(0) => break Ok(()),
                        Ok(_) => {
                            break Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "controller ownership pipe must carry no payload",
                            ));
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => break Err(error),
                    }
                };
                // The receiver may already be dropped after an OS exit request.
                let _ = sender.send(result);
            })?;
        Ok(Self { closed })
    }

    async fn closed(self) -> io::Result<()> {
        self.closed
            .await
            .map_err(|_| io::Error::other("controller ownership observer ended without a result"))?
    }
}

/// Acknowledges control startup, then requests the existing engine drain on
/// owner-pipe closure or an OS signal. `None` preserves standalone operation.
/// Call only after the control listener and its owners are prepared. This does
/// not assert that asynchronous audio-device startup has succeeded.
///
/// # Errors
/// Returns readiness I/O, owner-observation or signal errors. The caller must
/// still close admission and drain accepted work before releasing native owners.
pub async fn shutdown(
    owner: Option<EngineOwner>,
    signal: impl Future<Output = io::Result<()>>,
) -> io::Result<()> {
    let Some(owner) = owner else {
        return signal.await;
    };
    {
        let mut output = io::stdout().lock();
        output.write_all(crate::READY)?;
        output.flush()?;
    }
    tokio::select! {
        result = owner.closed() => result,
        result = signal => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn owner_eof_requests_clean_drain() {
        let owner = EngineOwner::from_reader(io::empty()).unwrap();
        assert!(owner.closed().await.is_ok());
    }

    #[tokio::test]
    async fn unexpected_payload_requests_failed_drain() {
        let owner = EngineOwner::from_reader(io::Cursor::new([1])).unwrap();
        assert_eq!(
            owner.closed().await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn standalone_preserves_signal_result() {
        let result = shutdown(None, async { Err(io::Error::other("signal failed")) }).await;
        assert_eq!(result.unwrap_err().to_string(), "signal failed");
    }
}
