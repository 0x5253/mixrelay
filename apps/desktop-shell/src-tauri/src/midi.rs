//! Resident input-only MIDI control, sharing the local controller's in-process boundary.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use mixrelay_midi_control::{Action, Binding, Control, Device, Mode, Target, assign};
use serde::{Deserialize, Serialize};

use crate::control_proxy::local::LocalControl;

pub(crate) mod admission;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod native;
mod store;
mod worker;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Learn {
    Idle,
    Waiting,
    Captured { control: Control },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Assignment {
    target: Target,
    action: Action,
    graph_name: String,
    node_name: String,
}

#[derive(Debug)]
struct Selection {
    assignment: Assignment,
    pending: Option<Binding>,
}

#[derive(Debug)]
pub(super) struct State {
    store: store::Store,
    devices: Vec<Device>,
    ports: Vec<(u32, Device)>,
    unavailable: Option<String>,
    preferences_error: Option<String>,
    learn: Learn,
    map_mode: bool,
    selection: Option<Selection>,
    snapshot: serde_json::Value,
    capture: Arc<mixrelay_midi_control::learn::LearnCapture>,
    mailboxes: Vec<Arc<mixrelay_midi_control::mailbox::Mailbox>>,
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

impl State {
    fn load(path: std::path::PathBuf) -> Self {
        Self::with_repository(store::repository(path))
    }
    fn with_repository(
        repository: Arc<dyn mixrelay_desktop_core::recording::PreferencesRepository>,
    ) -> Self {
        let (store, preferences_error) = match store::Store::load(Arc::clone(&repository)) {
            Ok(store) => (store, None),
            Err(error) => (
                store::Store::empty(repository),
                Some(format!("MIDI preferences unavailable: {error}")),
            ),
        };
        Self {
            store,
            devices: Vec::new(),
            ports: Vec::new(),
            unavailable: preferences_error.clone(),
            preferences_error,
            learn: Learn::Idle,
            map_mode: false,
            selection: None,
            snapshot: serde_json::Value::Null,
            capture: Arc::default(),
            mailboxes: Vec::new(),
        }
    }

    fn observe_learn(&mut self) -> Result<(), MidiError> {
        let Some((token, channel, kind, number)) = self.capture.captured() else {
            return Ok(());
        };
        let Some((_, device)) = self.ports.iter().find(|(id, _)| *id == token) else {
            return Ok(());
        };
        let Some(selection) = &self.selection else {
            return Ok(());
        };
        let control = Control {
            device: device.clone(),
            channel,
            kind,
            number,
        };
        if matches!(&self.learn, Learn::Captured { control: previous } if previous.matches(&control))
        {
            return Ok(());
        }
        if self.preferences_error.is_some() {
            return Err(MidiError::Preferences);
        }
        let assignment = &selection.assignment;
        if assignment.action == Action::Volume && kind != mixrelay_midi_control::Kind::Cc {
            return Ok(());
        }
        let binding = Binding {
            control: control.clone(),
            target: assignment.target.clone(),
            action: assignment.action,
            graph_name: assignment.graph_name.clone(),
            node_name: assignment.node_name.clone(),
            mode: Mode::Absolute,
            step_db: 1.0,
            min_db: -60.0,
            max_db: 12.0,
        };
        let mut bindings = self.store.bindings().to_vec();
        if let Some(pending) = &selection.pending {
            bindings.retain(|entry| !entry.matches(pending));
        }
        let added = !bindings.iter().any(|entry| entry.matches(&binding));
        assign(&mut bindings, binding.clone())?;
        // Publish capture only after its durable write succeeds.
        self.store.save(bindings)?;
        if let Some(selection) = &mut self.selection {
            selection.pending = added.then_some(binding);
        }
        self.learn = Learn::Captured { control };
        self.clear_input();
        Ok(())
    }

    fn edit(&mut self, arguments: &serde_json::Value) -> Result<(), MidiError> {
        if self.preferences_error.is_some() {
            return Err(MidiError::Preferences);
        }
        let binding: Binding = serde_json::from_value(arguments["binding"].clone())?;
        binding.validate()?;
        let identity: Binding = serde_json::from_value(arguments["identity"].clone())?;
        let mut bindings = self.store.bindings().to_vec();
        let index = bindings
            .iter()
            .position(|item| item.matches(&identity))
            .ok_or(MidiError::Learn)?;
        if bindings
            .iter()
            .enumerate()
            .any(|(other, item)| other != index && item.matches(&binding))
        {
            return Err(MidiError::Preferences);
        }
        bindings[index] = binding.clone();
        self.store.save(bindings)?;
        if let Some(selection) = &mut self.selection {
            if selection.assignment.target == identity.target
                && selection.assignment.action == identity.action
            {
                selection.assignment = Assignment {
                    target: binding.target.clone(),
                    action: binding.action,
                    graph_name: binding.graph_name.clone(),
                    node_name: binding.node_name.clone(),
                };
            }
            if selection
                .pending
                .as_ref()
                .is_some_and(|pending| pending.matches(&identity))
            {
                selection.pending = Some(binding);
            }
        }
        self.clear_input();
        Ok(())
    }

    fn refresh_names(&mut self, snapshot: &serde_json::Value) -> Result<(), MidiError> {
        let mut bindings = self.store.bindings().to_vec();
        for binding in &mut bindings {
            if let Some(node) =
                mixrelay_desktop_core::midi_commands::node(snapshot, &binding.target)
                && let Some(graph) = snapshot["graphs"].as_array().and_then(|graphs| {
                    graphs
                        .iter()
                        .find(|graph| graph["id"] == binding.target.graph_id)
                })
            {
                if let Some(graph_name) = graph["name"].as_str() {
                    binding.graph_name = graph_name.to_owned();
                }
                if let Some(node_name) = node["label"].as_str() {
                    binding.node_name = node_name.to_owned();
                }
            }
        }
        if bindings != *self.store.bindings() {
            self.store.save(bindings)?;
        }
        Ok(())
    }

    fn clear_input(&mut self) {
        self.store.invalidate();
        for mailbox in &self.mailboxes {
            mailbox.clear();
        }
    }
}

impl Midi {
    pub(crate) fn start(
        app: &tauri::AppHandle,
        control: Arc<LocalControl>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::load(
            crate::background::data_directory(app)?.join("midi-controls.json"),
        )));
        let stop = Arc::new(AtomicBool::new(false));
        let (completed, done) = tokio::sync::watch::channel(false);
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let operation_liveness = control.midi_liveness();
        let write_gate = Arc::new(tokio::sync::Mutex::new(()));
        let worker_gate = Arc::clone(&write_gate);
        std::thread::Builder::new()
            .name("midi-control".into())
            .spawn(move || {
                worker::run(
                    &worker_state,
                    &worker_stop,
                    &control,
                    &worker_gate,
                    &mixrelay_time::StdClock::new(),
                );
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

    pub(crate) async fn command(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, MidiError> {
        let _write = self.write_gate.lock().await;
        let mut state = lock(&self.state);
        state.observe_learn()?;
        match name {
            "midi_status" => {}
            "midi_map" => {
                state.clear_input();
                state.map_mode = arguments["enabled"]
                    .as_bool()
                    .ok_or(MidiError::Preferences)?;
                state.selection = None;
                state.learn = Learn::Idle;
                state.capture.cancel();
            }
            "midi_learn" => {
                if state.preferences_error.is_some() {
                    return Err(MidiError::Preferences);
                }
                if !state.map_mode {
                    return Err(MidiError::Learn);
                }
                let assignment = serde_json::from_value(arguments["assignment"].clone())?;
                state.clear_input();
                state.selection = Some(Selection {
                    assignment,
                    pending: None,
                });
                state.learn = Learn::Waiting;
                state.capture.arm();
            }
            "midi_cancel" => {
                state.clear_input();
                state.map_mode = false;
                state.selection = None;
                state.capture.cancel();
                state.learn = Learn::Idle;
            }
            "midi_save" => state.edit(arguments)?,
            "midi_remove" => {
                if state.preferences_error.is_some() {
                    return Err(MidiError::Preferences);
                }
                let identity: Binding = serde_json::from_value(arguments["binding"].clone())?;
                let bindings = state
                    .store
                    .bindings()
                    .iter()
                    .filter(|item| !item.matches(&identity))
                    .cloned()
                    .collect();
                state.store.save(bindings)?;
                state.clear_input();
                state.selection = None;
                state.capture.cancel();
                state.learn = Learn::Idle;
            }
            _ => return Err(MidiError::Preferences),
        }
        Ok(serde_json::json!({
            "bindings":state.store.bindings(), "devices":state.devices,
            "unavailable":state.unavailable, "learn":state.learn, "snapshot":state.snapshot, "mapMode":state.map_mode,
        }))
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
