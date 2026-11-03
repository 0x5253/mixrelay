//! Private file framing for isolated processor startup.

use crate::state::InitialState;
use mixrelay_processor_state::StateStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

// Startup parameters are bounded independently of the externally stored native payload.
const MAX_CONFIGURATION_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerConfiguration {
    sample_rate: u32,
    ports: mixrelay_audio_graph::ProviderPorts,
    parameters: Value,
    state_store: Option<PathBuf>,
}

pub(super) fn write_worker_config(
    path: &Path,
    sample_rate: u32,
    parameters: &Value,
    store: Option<&StateStore>,
    ports: &mixrelay_audio_graph::ProviderPorts,
) -> Result<(), String> {
    ports.validate().map_err(|error| error.to_string())?;
    let body = WorkerConfiguration {
        sample_rate,
        ports: ports.clone(),
        parameters: parameters.clone(),
        state_store: store.map(|store| store.root().to_path_buf()),
    };
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut file, &body).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())
}

pub(super) struct PreparedConfiguration {
    pub(super) sample_rate: f64,
    pub(super) ports: mixrelay_audio_graph::ProviderPorts,
    pub(super) initial: InitialState,
}

pub(super) fn read_worker_configuration(path: &str) -> Result<PreparedConfiguration, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(MAX_CONFIGURATION_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_CONFIGURATION_BYTES {
        return Err("processor configuration exceeds size limit".to_owned());
    }
    let config: WorkerConfiguration =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    config.ports.validate().map_err(|error| error.to_string())?;
    if config.sample_rate == 0
        || config
            .state_store
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
    {
        return Err("invalid processor startup configuration".to_owned());
    }
    let store = config.state_store.map(StateStore::new);
    let state = InitialState::resolve(
        config.parameters,
        store
            .as_ref()
            .map(|store| store as &dyn crate::StateBlobReader),
    )
    .map_err(|error| error.to_string())?;
    Ok(PreparedConfiguration {
        sample_rate: f64::from(config.sample_rate),
        ports: config.ports,
        initial: state,
    })
}

#[cfg(test)]
mod tests;
