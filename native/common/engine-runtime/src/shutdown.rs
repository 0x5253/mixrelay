//! Observes process shutdown signals before audio startup begins.

/// OS shutdown requests, caught from registration on, awaited on the control runtime and
/// never the audio thread. Registering before start-up work turns a stop during start-up
/// into an orderly shutdown instead of a killed process.
#[cfg(unix)]
pub struct ShutdownSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl ShutdownSignals {
    /// Must run inside a Tokio runtime.
    ///
    /// # Errors
    /// Returns signal registration failures.
    pub fn register() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    /// Resolves at the first shutdown request, including one received before this call.
    ///
    /// # Errors
    /// Never fails today; kept fallible like other shutdown sources.
    pub async fn requested(mut self) -> std::io::Result<()> {
        tokio::select! {
            _ = self.terminate.recv() => Ok(()),
            _ = self.interrupt.recv() => Ok(()),
        }
    }
}

#[cfg(not(unix))]
pub struct ShutdownSignals;

#[cfg(not(unix))]
impl ShutdownSignals {
    /// # Errors
    /// Never fails on this platform.
    pub fn register() -> std::io::Result<Self> {
        Ok(Self)
    }

    /// # Errors
    /// Returns Ctrl-C registration failures.
    pub async fn requested(self) -> std::io::Result<()> {
        tokio::signal::ctrl_c().await
    }
}

#[cfg(all(test, unix))]
mod shutdown_signal_tests {
    use super::ShutdownSignals;

    #[tokio::test]
    async fn a_stop_received_before_waiting_still_ends_the_service() {
        let signals = ShutdownSignals::register().unwrap();
        // SAFETY: the registered Tokio handler replaces the default disposition, so the
        // signal is recorded for this test instead of ending the process.
        assert_eq!(unsafe { libc::raise(libc::SIGINT) }, 0);
        signals.requested().await.unwrap();
        let signals = ShutdownSignals::register().unwrap();
        // SAFETY: as above, for the termination request launchd sends.
        assert_eq!(unsafe { libc::raise(libc::SIGTERM) }, 0);
        signals.requested().await.unwrap();
    }
}
