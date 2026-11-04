//! Hardware-independent MIDI binding, learn and soft-takeover policy.

pub mod learn;
pub mod mailbox;
mod relative;
pub mod router;
pub mod runtime;

use serde::{Deserialize, Serialize};

/// One physical input; names remain available when a native ID cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// Supported channel voice messages. Channels are zero-based on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Note,
    Cc,
}

/// A hardware input can drive several independent assignments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub device: Device,
    pub channel: u8,
    pub kind: Kind,
    pub number: u8,
}

impl Control {
    /// Compare stable identities; a remembered display name can change.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.device.id == other.device.id
            && self.channel == other.channel
            && self.kind == other.kind
            && self.number == other.number
    }
}

/// Discrete commands mirror the desktop's existing node and recorder actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Power,
    Mute,
    Volume,
    Record,
    Pause,
    Stop,
}

/// Encoder encodings use signed seven-bit deltas; zero/64 are neutral respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Absolute,
    TwosComplement,
    Offset64,
}

/// The graph and node identity survive deletion until explicitly unbound.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub graph_id: String,
    pub node_id: String,
}

/// A durable assignment, independent of connected devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub control: Control,
    pub target: Target,
    pub action: Action,
    pub mode: Mode,
    pub step_db: f32,
    pub graph_name: String,
    pub node_name: String,
    pub min_db: f32,
    pub max_db: f32,
}

/// Persistence and native command input share the same validation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindingError {
    #[error("invalid MIDI binding")]
    Invalid,
}

impl Binding {
    /// Options and remembered names are not part of a mapping's identity.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.control.matches(&other.control)
            && self.target == other.target
            && self.action == other.action
    }

    /// # Errors
    /// Rejects invalid wire values, identities, options and Note volume assignments.
    pub fn validate(&self) -> Result<(), BindingError> {
        let identities = [
            &self.control.device.id,
            &self.control.device.name,
            &self.target.graph_id,
            &self.target.node_id,
            &self.graph_name,
            &self.node_name,
        ];
        if self.control.channel > 15
            || self.control.number > 127
            || identities
                .iter()
                .any(|value| value.is_empty() || value.len() > 1024 || value.contains('\0'))
            || ![0.5, 1.0, 2.0, 3.0].contains(&self.step_db)
            || !self.min_db.is_finite()
            || !self.max_db.is_finite()
            || !(-60.0..=12.0).contains(&self.min_db)
            || !(-60.0..=12.0).contains(&self.max_db)
            || (self.action == Action::Volume && self.control.kind != Kind::Cc)
        {
            return Err(BindingError::Invalid);
        }
        Ok(())
    }
}

/// Add a mapping; an exact duplicate retains its existing options.
/// # Errors
/// Returns validation failures without changing the list.
pub fn assign(bindings: &mut Vec<Binding>, binding: Binding) -> Result<(), BindingError> {
    binding.validate()?;
    if !bindings.iter().any(|item| item.matches(&binding)) {
        bindings.push(binding);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
