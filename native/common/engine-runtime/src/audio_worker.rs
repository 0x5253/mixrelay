//! Owns cancellation and off-thread joining of the audio worker.

use crate::join_worker;
use mixrelay_engine::session::AdapterRuntimeStatus;
use std::sync::Arc;

/// Control-plane ownership of the audio thread. Bind/startup failures and normal
/// server shutdown request cancellation and join before platform resources die.
/// Never construct or destroy this owner on an audio callback. A stuck native
/// driver call still requires process isolation; Rust cannot safely kill a thread.
pub struct AudioWorker {
    status: Arc<AdapterRuntimeStatus>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AudioWorker {
    /// Takes ownership of `thread` and its cancellation status until joined off-RT.
    #[must_use]
    pub fn new(status: Arc<AdapterRuntimeStatus>, thread: std::thread::JoinHandle<()>) -> Self {
        Self {
            status,
            thread: Some(thread),
        }
    }

    /// Requests cancellation and waits for the audio worker to release its owners.
    /// Must be called off the audio thread.
    ///
    /// # Errors
    /// Returns the worker identity and panic reason if the audio thread panics.
    pub fn finish(mut self) -> Result<(), String> {
        self.join()
    }

    fn join(&mut self) -> Result<(), String> {
        self.status.request_stop();
        self.thread.take().map_or(Ok(()), join_worker)
    }
}

impl Drop for AudioWorker {
    fn drop(&mut self) {
        // Cancellation/startup unwind must still join; finish propagates errors.
        let _ = self.join();
    }
}
