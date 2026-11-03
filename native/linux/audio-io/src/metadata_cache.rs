//! Owned metadata cache with explicit file, environment and monotonic-clock inputs.
use mixrelay_time::{Instant, MonotonicClock};
use std::{path::PathBuf, sync::Mutex, time::Duration};

const MAX_CACHE_ENTRIES: usize = 64;
const CACHE_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub display_name: Option<String>,
    pub icon_data_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    pub application_id: Option<String>,
    pub executable: Option<String>,
    pub icon_name: Option<String>,
    pub theme_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Environment {
    pub roots: Vec<PathBuf>,
    pub locale: Option<String>,
    pub theme_name: Option<String>,
}

pub(crate) trait MetadataEnvironment {
    fn snapshot(&self) -> Environment;
}

pub(crate) trait MetadataFiles {
    fn lookup(&self, key: &Key, roots: &[PathBuf], locale: Option<&str>) -> Metadata;
}

struct Entry {
    key: Key,
    environment: Environment,
    metadata: Metadata,
    created: Instant,
}

pub(crate) struct Resolver<C, E, F> {
    clock: C,
    environment: E,
    files: F,
    entries: Mutex<Vec<Entry>>,
}

impl<C: MonotonicClock, E: MetadataEnvironment, F: MetadataFiles> Resolver<C, E, F> {
    pub(crate) const fn new(clock: C, environment: E, files: F) -> Self {
        Self {
            clock,
            environment,
            files,
            entries: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn resolve(&self, mut key: Key) -> Metadata {
        let environment = self.environment.snapshot();
        key.theme_name.clone_from(&environment.theme_name);
        let now = self.clock.now();
        {
            let entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = entries.iter().find(|entry| {
                entry.key == key
                    && entry.environment == environment
                    && now.duration_since(entry.created) < CACHE_LIFETIME
            }) {
                return entry.metadata.clone();
            }
        }
        // Disk lookup runs outside the cache lock so unrelated requests can progress.
        let metadata = self
            .files
            .lookup(&key, &environment.roots, environment.locale.as_deref());
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .retain(|entry| entry.key != key && now.duration_since(entry.created) < CACHE_LIFETIME);
        if entries.len() == MAX_CACHE_ENTRIES {
            entries.remove(0);
        }
        entries.push(Entry {
            key,
            environment,
            metadata: metadata.clone(),
            created: self.clock.now(),
        });
        metadata
    }
}

#[cfg(test)]
mod tests;
