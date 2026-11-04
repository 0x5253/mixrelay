//! A separate, exact-device tap suppresses an app's direct playback without narrowing the
//! shared capture.
use super::capture_error::{CaptureOperation, CaptureProbeError};
use super::clock_pump::ports::PlaybackLease;
use super::process_tap::ProcessTap;
use std::sync::{Mutex, MutexGuard};

pub(super) struct ScopedPlayback(Mutex<Option<ProcessTap>>);

impl ScopedPlayback {
    pub(super) fn new(tap: ProcessTap) -> Self {
        Self(Mutex::new(Some(tap)))
    }

    /// Starts suppression; `false` when it was already running.
    pub(super) fn start(&self) -> Result<bool, String> {
        self.refresh()?;
        let owner = self.owner()?;
        let tap = owner.as_ref().ok_or("scoped playback lease was revoked")?;
        if tap.stats().running {
            return Ok(false);
        }
        tap.start_native().map_err(|error| error.to_string())?;
        Ok(true)
    }

    /// The mutex is the tap's sole control owner; no ring reader uses this tap.
    fn owner(&self) -> Result<MutexGuard<'_, Option<ProcessTap>>, String> {
        self.0
            .lock()
            .map_err(|_| "scoped playback owner is poisoned".to_owned())
    }
}

impl PlaybackLease for ScopedPlayback {
    fn refresh(&self) -> Result<(), String> {
        let mut owner = self.owner()?;
        let tap = owner.as_ref().ok_or("scoped playback lease was revoked")?;
        let Err(error) = tap.refresh_native() else {
            return Ok(());
        };
        // Destroy transfers this exact tap to native retirement; retaining
        // the empty owner prevents later ACK retries from restoring its lease.
        drop(owner.take());
        Err(CaptureProbeError::new(CaptureOperation::Refresh, error).to_string())
    }

    fn stop(&self) -> Result<(), String> {
        let owner = self.owner()?;
        match owner.as_ref() {
            Some(tap) if tap.stats().running => {
                tap.stop_native().map_err(|error| error.to_string())
            }
            _ => Ok(()),
        }
    }
}
