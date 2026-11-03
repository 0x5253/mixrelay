//! Native composition for the versioned MIDI binding repository.
pub(super) use mixrelay_desktop_core::midi_preferences::BindingStore as Store;
use mixrelay_desktop_core::midi_preferences::FILE_LIMIT;
use std::{path::PathBuf, sync::Arc};

pub(super) fn repository(
    path: PathBuf,
) -> Arc<dyn mixrelay_desktop_core::recording::PreferencesRepository> {
    Arc::new(crate::preferences_file::AtomicPreferences::new(
        path, FILE_LIMIT,
    ))
}
#[cfg(test)]
mod tests;
