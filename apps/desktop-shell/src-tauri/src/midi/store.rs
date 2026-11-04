//! Native composition for the versioned MIDI binding repository.
use mixrelay_desktop_core::{midi_preferences::FILE_LIMIT, recording::PreferencesRepository};
use std::{path::PathBuf, sync::Arc};

pub(super) fn repository(path: PathBuf) -> Arc<dyn PreferencesRepository> {
    Arc::new(crate::preferences_file::AtomicPreferences::new(
        path, FILE_LIMIT,
    ))
}

#[cfg(test)]
mod tests;
