//! Versioned, atomic desktop preferences; failed writes never publish new bindings.
use crate::recording::PreferencesRepository;
use std::sync::Arc;

use mixrelay_midi_control::{Binding, assign};
use serde::{Deserialize, Serialize};

/// MIDI preferences failures retain binding, JSON and storage causes.
#[derive(Debug, thiserror::Error)]
pub enum MidiPreferencesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Binding(#[from] mixrelay_midi_control::BindingError),
    #[error("invalid MIDI preferences")]
    Preferences,
}

// Same 1MiB admission limit as the desktop recorder preferences.
pub const FILE_LIMIT: usize = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u32,
    bindings: Vec<Binding>,
}

#[derive(Debug)]
pub struct BindingStore {
    repository: Arc<dyn PreferencesRepository>,
    bindings: Vec<Binding>,
    generation: u64,
}

impl BindingStore {
    /// An empty fallback retains the repository without overwriting unreadable data.
    #[must_use]
    pub fn empty(repository: Arc<dyn PreferencesRepository>) -> Self {
        Self {
            repository,
            bindings: Vec::new(),
            generation: 0,
        }
    }

    /// Loads bounded, versioned bindings without requiring connected devices or targets.
    /// # Errors
    /// Returns storage, document, or binding validation failures without writing.
    pub fn load(repository: Arc<dyn PreferencesRepository>) -> Result<Self, MidiPreferencesError> {
        let mut bindings = Vec::new();
        if let Some(bytes) = repository.read()? {
            if bytes.len() > FILE_LIMIT {
                return Err(MidiPreferencesError::Preferences);
            }
            let saved: Saved = serde_json::from_slice(&bytes)?;
            if saved.version != 1 {
                return Err(MidiPreferencesError::Preferences);
            }
            for binding in saved.bindings {
                assign(&mut bindings, binding)?;
            }
        }
        Ok(Self {
            repository,
            bindings,
            generation: 0,
        })
    }

    /// Current bindings, published only after the repository commits.
    #[must_use]
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }
    /// Invalidates queued input after settings or learning changes.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    /// Generation captured by admitted work.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Commits a complete validated binding set before publishing it.
    /// # Errors
    /// Returns storage, JSON, or binding failures with the previous state intact.
    pub fn save(&mut self, bindings: Vec<Binding>) -> Result<(), MidiPreferencesError> {
        for binding in &bindings {
            binding.validate()?;
        }
        let bytes = serde_json::to_vec(&Saved {
            version: 1,
            bindings: bindings.clone(),
        })?;
        if bytes.len() > FILE_LIMIT {
            return Err(MidiPreferencesError::Preferences);
        }
        self.repository.replace(&bytes)?;
        self.bindings = bindings;
        self.invalidate();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
