//! Resident input-only MIDI control, sharing the local controller's in-process boundary.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::control_proxy::local::LocalControl;

pub(crate) mod admission;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native;
mod state;
mod store;
mod worker;

use state::{Command, State};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MidiError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Binding(#[from] mixrelay_midi_control::BindingError),
    #[error(transparent)]
    Stored(#[from] mixrelay_desktop_core::midi_preferences::MidiPreferencesError),
    #[error("invalid MIDI preferences")]
    Preferences,
    #[error("MIDI learn is no longer available")]
    Learn,
}

/// The native worker owns every connection and drops them before signalling shutdown.
pub(crate) struct Midi {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    done: tokio::sync::watch::Receiver<bool>,
    operation_liveness: Arc<tokio::sync::Notify>,
    write_gate: Arc<tokio::sync::Mutex<()>>,
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Midi {
    pub(crate) fn start(
        app: &tauri::AppHandle,
        control: Arc<LocalControl>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path = crate::background::data_directory(app)?.join("midi-controls.json");
        let state = Arc::new(Mutex::new(State::with_repository(store::repository(path))));
        let stop = Arc::new(AtomicBool::new(false));
        let (completed, done) = tokio::sync::watch::channel(false);
        let operation_liveness = control.midi_liveness();
        let write_gate = Arc::new(tokio::sync::Mutex::new(()));
        let (worker_state, worker_stop, worker_gate) = (
            Arc::clone(&state),
            Arc::clone(&stop),
            Arc::clone(&write_gate),
        );
        std::thread::Builder::new()
            .name("midi-control".into())
            .spawn(move || {
                let shared = worker::Shared {
                    state: &worker_state,
                    stop: &worker_stop,
                    write_gate: &worker_gate,
                };
                let clock = mixrelay_time::StdClock::new();
                worker::Worker::new(shared, worker::NativeInputs::default(), &*control, &clock)
                    .run();
                let _ = completed.send(true);
            })?;
        Ok(Self {
            state,
            stop,
            done,
            operation_liveness,
            write_gate,
        })
    }

    pub(crate) async fn shutdown(&self) -> Result<(), String> {
        // Release/Acquire publishes stop before the worker drops native callbacks.
        self.stop.store(true, Ordering::Release);
        let mut done = self.done.clone();
        crate::control_proxy::wait_for_controls(
            async {
                while !*done.borrow() {
                    done.changed().await.map_err(|error| error.to_string())?;
                }
                Ok::<(), String>(())
            },
            &self.operation_liveness,
        )
        .await?
    }

    /// Runs an editor command after observing any capture that arrived since the last one.
    pub(crate) async fn command(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, MidiError> {
        let _write = self.write_gate.lock().await;
        let mut state = lock(&self.state);
        state.observe_learn()?;
        let command = Command::parse(name).ok_or(MidiError::Preferences)?;
        state.apply(command, arguments)
    }
}

/// Dismiss learn before the editor hides/exits; resident bindings then keep running.
pub(crate) async fn cancel_editor_learn(app: &tauri::AppHandle) {
    use tauri::Manager as _;
    if let Some(midi) = app.try_state::<Midi>() {
        if let Err(error) = midi.command("midi_cancel", &serde_json::Value::Null).await {
            tracing::warn!(%error, "could not cancel MIDI learn on editor close");
        }
    } else if let Some(connection) = app.try_state::<crate::background::Connection>()
        && let Err(error) = connection
            .invoke("midi_cancel", serde_json::Value::Null)
            .await
    {
        tracing::warn!(?error, "could not cancel MIDI learn on editor close");
    }
}

impl Drop for Midi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) mod tests;
