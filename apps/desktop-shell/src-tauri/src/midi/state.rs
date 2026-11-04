//! Durable bindings, learn progress and the editor's MIDI commands.
use std::sync::Arc;

use mixrelay_desktop_core::{midi_preferences::BindingStore, recording::PreferencesRepository};
use mixrelay_midi_control::{
    Action, Binding, Control, Device, Kind, Mode, Target, assign, learn::LearnCapture,
    mailbox::Mailbox,
};
use serde::{Deserialize, Serialize};

use super::MidiError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Learn {
    Idle,
    Waiting,
    Captured { control: Control },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Assignment {
    target: Target,
    action: Action,
    graph_name: String,
    node_name: String,
}

impl Assignment {
    fn of(binding: &Binding) -> Self {
        Self {
            target: binding.target.clone(),
            action: binding.action,
            graph_name: binding.graph_name.clone(),
            node_name: binding.node_name.clone(),
        }
    }

    /// The binding a captured control creates. Volume takes absolute CC values over the
    /// node's full gain range.
    fn binding(&self, control: Control) -> Binding {
        Binding {
            control,
            target: self.target.clone(),
            action: self.action,
            graph_name: self.graph_name.clone(),
            node_name: self.node_name.clone(),
            mode: Mode::Absolute,
            step_db: 1.0,
            min_db: -60.0,
            max_db: 12.0,
        }
    }
}

/// The node being learned and the binding its latest capture added, which the next
/// capture replaces.
#[derive(Debug)]
pub(super) struct Selection {
    pub(super) assignment: Assignment,
    pub(super) pending: Option<Binding>,
}

/// An editor request, named by its command string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    Status,
    Map,
    Learn,
    Cancel,
    Save,
    Remove,
}

impl Command {
    pub(super) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "midi_status" => Self::Status,
            "midi_map" => Self::Map,
            "midi_learn" => Self::Learn,
            "midi_cancel" => Self::Cancel,
            "midi_save" => Self::Save,
            "midi_remove" => Self::Remove,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub(super) struct State {
    pub(super) store: BindingStore,
    pub(super) devices: Vec<Device>,
    /// Open ports by the token their callbacks report to learn capture.
    pub(super) ports: Vec<(u32, Device)>,
    pub(super) unavailable: Option<String>,
    /// Unreadable preferences stay on disk; nothing may overwrite them.
    pub(super) preferences_error: Option<String>,
    pub(super) learn: Learn,
    pub(super) map_mode: bool,
    pub(super) selection: Option<Selection>,
    pub(super) snapshot: serde_json::Value,
    pub(super) capture: Arc<LearnCapture>,
    pub(super) mailboxes: Vec<Arc<Mailbox>>,
}

impl State {
    pub(super) fn with_repository(repository: Arc<dyn PreferencesRepository>) -> Self {
        let (store, preferences_error) = match BindingStore::load(Arc::clone(&repository)) {
            Ok(store) => (store, None),
            Err(error) => (
                BindingStore::empty(repository),
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

    /// Runs one editor command and returns the status the editor renders.
    pub(super) fn apply(
        &mut self,
        command: Command,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, MidiError> {
        match command {
            Command::Status => {}
            Command::Map => self.set_map_mode(arguments)?,
            Command::Learn => self.select(arguments)?,
            Command::Cancel => self.cancel(),
            Command::Save => self.edit(arguments)?,
            Command::Remove => self.remove(arguments)?,
        }
        Ok(self.status())
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "bindings": self.store.bindings(),
            "devices": self.devices,
            "unavailable": self.unavailable,
            "learn": self.learn,
            "snapshot": self.snapshot,
            "mapMode": self.map_mode,
        })
    }

    fn set_map_mode(&mut self, arguments: &serde_json::Value) -> Result<(), MidiError> {
        self.clear_input();
        self.map_mode = arguments["enabled"]
            .as_bool()
            .ok_or(MidiError::Preferences)?;
        self.end_learn();
        Ok(())
    }

    fn select(&mut self, arguments: &serde_json::Value) -> Result<(), MidiError> {
        self.ensure_writable()?;
        if !self.map_mode {
            return Err(MidiError::Learn);
        }
        let assignment = serde_json::from_value(arguments["assignment"].clone())?;
        self.clear_input();
        self.selection = Some(Selection {
            assignment,
            pending: None,
        });
        self.learn = Learn::Waiting;
        self.capture.arm();
        Ok(())
    }

    fn cancel(&mut self) {
        self.clear_input();
        self.map_mode = false;
        self.end_learn();
    }

    fn remove(&mut self, arguments: &serde_json::Value) -> Result<(), MidiError> {
        self.ensure_writable()?;
        let identity: Binding = serde_json::from_value(arguments["binding"].clone())?;
        let bindings = self
            .store
            .bindings()
            .iter()
            .filter(|item| !item.matches(&identity))
            .cloned()
            .collect();
        self.store.save(bindings)?;
        self.clear_input();
        self.end_learn();
        Ok(())
    }

    fn end_learn(&mut self) {
        self.selection = None;
        self.capture.cancel();
        self.learn = Learn::Idle;
    }

    fn ensure_writable(&self) -> Result<(), MidiError> {
        if self.preferences_error.is_some() {
            return Err(MidiError::Preferences);
        }
        Ok(())
    }

    /// Turns the latest captured control into a saved binding for the selected node.
    pub(super) fn observe_learn(&mut self) -> Result<(), MidiError> {
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
        self.ensure_writable()?;
        let assignment = &selection.assignment;
        if assignment.action == Action::Volume && kind != Kind::Cc {
            return Ok(());
        }
        let binding = assignment.binding(control.clone());
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

    /// Replaces the binding named by `identity`; the selection follows the edit.
    fn edit(&mut self, arguments: &serde_json::Value) -> Result<(), MidiError> {
        self.ensure_writable()?;
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
                selection.assignment = Assignment::of(&binding);
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

    /// Remembers current graph and node names; deleted targets keep their last names.
    pub(super) fn refresh_names(&mut self, snapshot: &serde_json::Value) -> Result<(), MidiError> {
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

    /// Starts a new mapping generation and drops every queued input.
    pub(super) fn clear_input(&mut self) {
        self.store.invalidate();
        for mailbox in &self.mailboxes {
            mailbox.clear();
        }
    }
}
