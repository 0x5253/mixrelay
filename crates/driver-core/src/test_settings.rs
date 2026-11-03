//! In-memory persistence with explicit read and write failures for Driver tests.

use std::{io, sync::Arc};

use parking_lot::Mutex;

use crate::{DriverError, SettingsStore, StoredSettings};

#[derive(Debug, Clone, Default)]
pub struct MemorySettings(pub Arc<Mutex<Observation>>);

#[derive(Debug, Default)]
pub struct Observation {
    pub saved: Option<StoredSettings>,
    pub fail_load: bool,
    pub fail_save: bool,
    pub writes: usize,
}

impl SettingsStore for MemorySettings {
    fn load(&self) -> Result<Option<StoredSettings>, DriverError> {
        let observation = self.0.lock();
        if observation.fail_load {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
        Ok(observation.saved.clone())
    }

    fn save(&self, settings: &StoredSettings) -> Result<(), DriverError> {
        let mut observation = self.0.lock();
        observation.writes += 1;
        if observation.fail_save {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
        observation.saved = Some(settings.clone());
        Ok(())
    }
}
