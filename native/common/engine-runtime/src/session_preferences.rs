//! Typed audio-session settings. Filesystem replacement and configuration paths
//! belong to persistence; this module validates the session document contract.

use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
};

use mixrelay_audio_io::session::{AudioBackendKind, SessionFormat};
use serde::{Deserialize, Serialize};

use crate::persistence::{StagedWrite, config_directory, stage_json};

const MAX_BYTES: u64 = 4096;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    version: u32,
    backend: AudioBackendKind,
    format: SessionFormat,
}

/// A backend-specific session document at an application configuration path.
#[derive(Debug)]
pub struct SessionPreferences {
    path: PathBuf,
    backend: AudioBackendKind,
}

impl SessionPreferences {
    #[must_use]
    pub fn at(path: PathBuf, backend: AudioBackendKind) -> Self {
        Self { path, backend }
    }

    /// Resolves this backend's settings path without reading or creating a file.
    /// # Errors
    /// Returns configuration-directory lookup errors.
    pub fn for_backend(backend: AudioBackendKind) -> io::Result<Self> {
        Ok(Self {
            path: config_directory()?.join("audio-session.json"),
            backend,
        })
    }

    /// Reads a bounded, version/backend-checked document; absence returns `None`.
    /// # Errors
    /// Returns read, JSON, size-limit or document-contract errors.
    pub fn load(&self) -> io::Result<Option<SessionFormat>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "session preferences exceed size limit",
            ));
        }
        let value: Preferences = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if value.version != 1 || value.backend != self.backend {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported session preferences version or backend",
            ));
        }
        Ok(Some(value.format))
    }

    /// Stages the session document without accepting it. The caller owns native
    /// format validation and when to commit.
    /// # Errors
    /// Returns file-staging errors.
    pub fn stage(&self, format: SessionFormat) -> io::Result<StagedWrite> {
        stage_json(
            &self.path,
            &Preferences {
                version: 1,
                backend: self.backend,
                format,
            },
        )
    }
}

impl mixrelay_engine::session_settings::SessionPreferences for SessionPreferences {
    fn load(&self) -> io::Result<Option<SessionFormat>> {
        Self::load(self)
    }
    fn stage(
        &self,
        format: SessionFormat,
    ) -> io::Result<Box<dyn mixrelay_engine::session_settings::StagedSessionPreferences>> {
        Ok(Box::new(Self::stage(self, format)?))
    }
}
impl mixrelay_engine::session_settings::StagedSessionPreferences for StagedWrite {
    fn commit(self: Box<Self>) -> io::Result<()> {
        (*self).commit()
    }
}
