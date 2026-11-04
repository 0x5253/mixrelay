//! Application capture: the desktop commands, and the owners of each engine's application
//! sources from preparation through acknowledgement.

use std::{
    collections::BTreeMap,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use mixrelay_audio_io::{AudioDeviceDescriptor, CapturableApplication};
use mixrelay_desktop_core::capture::ownership::{self as capture_transaction, CaptureOwners};
use tauri::State;

mod application_inventory;
mod application_source;
mod capture_backend;
mod capture_error;
mod clock_pump;
mod device_activity;
mod process_tap;
mod scoped_playback;
mod source_transport;
#[cfg(test)]
mod tests;

use application_source::ApplicationSource;
use capture_backend::MacApplicationCaptureBackend;
use clock_pump::ports::PlaybackLease;
pub(crate) use device_activity::DeviceActivityCache;
use process_tap::ProcessTap;

/// Frames each tap ring, converter and shared source ring holds.
const CAPTURE_RING_CAPACITY_FRAMES: usize = mixrelay_audio_io::retention::RING_FRAMES as usize;

/// A capture probe's native telemetry, as the probe commands report it.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaptureProbeStats {
    callback_count: u64,
    captured_frames: u64,
    dropped_frames: u64,
    available_frames: usize,
    channels: u32,
    sample_rate: f64,
    running: bool,
    format_ready: bool,
    format_generation: u64,
    clock_discontinuities: u64,
    sample_time: f64,
    host_time: u64,
    clock_valid: bool,
}

impl From<mixrelay_macos_process_tap::TapStats> for CaptureProbeStats {
    fn from(stats: mixrelay_macos_process_tap::TapStats) -> Self {
        Self {
            callback_count: stats.callback_count,
            captured_frames: stats.captured_frames,
            dropped_frames: stats.dropped_frames,
            available_frames: stats.available_frames,
            channels: stats.channels,
            sample_rate: stats.sample_rate,
            running: stats.running,
            format_ready: stats.format_ready,
            format_generation: stats.format_generation,
            clock_discontinuities: stats.clock_discontinuities,
            sample_time: stats.sample_time,
            host_time: stats.host_time,
            clock_valid: stats.clock_valid,
        }
    }
}

#[derive(Default)]
pub(crate) struct CaptureProbeState {
    session: Mutex<Option<ProcessTap>>,
}

impl CaptureProbeState {
    fn session(&self) -> Result<MutexGuard<'_, Option<ProcessTap>>, String> {
        self.session
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())
    }
}

pub(crate) struct PreparedApplicationCaptures {
    set: capture_transaction::CaptureSet<ApplicationSource>,
}

#[derive(Default)]
pub(crate) struct ApplicationCaptureState {
    owners: Mutex<CaptureOwners<ApplicationSource>>,
    backend: MacApplicationCaptureBackend,
    pub(crate) monitor_active: AtomicBool,
    /// Counts engines; observations taken before a release describe the lost engine.
    engine_generation: AtomicU64,
}

impl ApplicationCaptureState {
    fn owners(&self) -> Result<MutexGuard<'_, CaptureOwners<ApplicationSource>>, String> {
        self.owners
            .lock()
            .map_err(|_| "application capture state is poisoned".to_owned())
    }

    /// Prepares a manifest's sources against the latest retained owners.
    fn prepare_set(
        &self,
        manifest: &mixrelay_control_protocol::ApplicationCaptureManifest,
        owners: &CaptureOwners<ApplicationSource>,
    ) -> Result<capture_transaction::CaptureSet<ApplicationSource>, String> {
        let retained = owners.retained().map_err(|error| error.to_string())?;
        capture_transaction::prepare(
            manifest,
            owners.latest(),
            &retained,
            |id, request, previous| {
                ApplicationSource::prepare(&self.backend, id, request, previous)
            },
        )
        .map_err(|error| error.to_string())
    }

    pub(crate) fn reserve(
        &self,
        manifest: &mixrelay_control_protocol::ApplicationCaptureManifest,
    ) -> Result<(), String> {
        let mut owners = self.owners()?;
        owners
            .ensure_reservable()
            .map_err(|error| error.to_string())?;
        let set = self.prepare_set(manifest, &owners)?;
        owners.reserve(set).map_err(|error| error.to_string())
    }

    pub(crate) fn accept_reserved(&self) -> Result<(), String> {
        let mut owners = self.owners()?;
        let retired = owners
            .accept_reserved()
            .map_err(|error| error.to_string())?;
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn reject_reserved(&self) -> Result<(), String> {
        let mut owners = self.owners()?;
        let retired = owners.reject_reserved();
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn prepare(
        &self,
        manifest: &mixrelay_control_protocol::ApplicationCaptureManifest,
    ) -> Result<PreparedApplicationCaptures, String> {
        let owners = self.owners()?;
        let set = self.prepare_set(manifest, &owners)?;
        Ok(PreparedApplicationCaptures { set })
    }

    pub(crate) fn stage(&self, prepared: PreparedApplicationCaptures) -> Result<(), String> {
        let mut owners = self.owners()?;
        let retired = owners
            .stage(prepared.set)
            .map_err(|error| error.to_string())?;
        drop(owners);
        drop(retired);
        Ok(())
    }

    pub(crate) fn acknowledge(&self, revision: u64) -> Result<(), String> {
        let mut owners = self.owners()?;
        let retired = owners
            .acknowledge_started(
                revision,
                ApplicationSource::start_playback,
                |playback| playback.stop(),
                ApplicationSource::activate,
            )
            .map_err(|error| error.to_string())?;
        drop(owners);
        // Destruction joins native pump workers only on this blocking control thread.
        drop(retired);
        Ok(())
    }

    /// Drops every owner once the engine is lost: their transports map that engine's
    /// memory, and its successor publishes its own revisions. Joins pump workers, so
    /// call it on a blocking thread.
    pub(crate) fn release_all(&self) -> Result<(), String> {
        let mut owners = self.owners()?;
        self.engine_generation.fetch_add(1, Ordering::AcqRel);
        let released = std::mem::take(&mut *owners);
        drop(owners);
        drop(released);
        Ok(())
    }

    pub(crate) fn engine_generation(&self) -> u64 {
        self.engine_generation.load(Ordering::Acquire)
    }

    pub(crate) fn has_pending(&self) -> Result<bool, String> {
        Ok(self.owners()?.has_unacknowledged())
    }

    pub(crate) fn requires_manifest(&self, submitted_revision: u64) -> Result<bool, String> {
        Ok(self.owners()?.requires_manifest(submitted_revision))
    }
}

/// Apps currently playing into or recording from each `MixRelay` device, keyed by the
/// device's native UID. A device absent from the map has no app using it.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri injects State by value; a borrowed State is not a command argument"
)]
pub(crate) fn list_device_app_activity(
    cache: State<'_, DeviceActivityCache>,
) -> Result<BTreeMap<String, Vec<device_activity::DeviceAppActivity>>, String> {
    cache.activity().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_capturable_applications() -> Result<Vec<CapturableApplication>, String> {
    application_inventory::capturable_applications().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_audio_devices() -> Result<Vec<AudioDeviceDescriptor>, String> {
    let mut devices = mixrelay_macos_audio_io::devices().map_err(|error| error.to_string())?;
    devices.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.uid.cmp(&right.uid))
    });
    Ok(devices)
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri deserializes owned command arguments and State extractors"
)]
pub(crate) fn start_capture_probe(
    bundle_id: String,
    mute_while_captured: bool,
    state: State<'_, CaptureProbeState>,
) -> Result<CaptureProbeStats, String> {
    if mute_while_captured {
        return Err(
            "application source mute requires a verified MixRelay-owned forwarding endpoint"
                .to_owned(),
        );
    }
    let mut session = state.session()?;
    if session.is_some() {
        return Err("an application capture probe is already running".to_owned());
    }
    let capture = ProcessTap::start(&bundle_id, None).map_err(|error| error.to_string())?;
    let snapshot = capture.stats().into();
    *session = Some(capture);
    Ok(snapshot)
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command State is an owned request extractor"
)]
pub(crate) fn capture_probe_stats(
    state: State<'_, CaptureProbeState>,
) -> Result<CaptureProbeStats, String> {
    state
        .session()?
        .as_ref()
        .map(|capture| capture.stats().into())
        .ok_or_else(|| "no application capture probe is running".to_owned())
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command State is an owned request extractor"
)]
pub(crate) fn stop_capture_probe(state: State<'_, CaptureProbeState>) -> Result<(), String> {
    state
        .session()?
        .take()
        .map(drop)
        .ok_or_else(|| "no application capture probe is running".to_owned())
}
