//! Bounded support-report projection and per-owner export admission.
use mixrelay_driver_control::DriverSnapshot;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub mod editor_context;
pub use editor_context::EditorContext;

/// Reports contain at most sixty-four devices and a quarter MiB of serialized facts.
pub const REPORT_LIMIT: usize = 256 * 1024;
const DEVICE_LIMIT: usize = 64;

/// Explicit build, host, and timestamp facts captured at the native export boundary.
pub struct ReportEnvironment<'a> {
    pub build_version: &'a str,
    pub platform: &'a str,
    pub architecture: &'a str,
    pub os_version: &'a str,
    pub created_unix_seconds: Option<u64>,
}

/// Admission belongs to one desktop runtime, independent of native menu handles.
#[derive(Default)]
pub struct ExportAdmission(AtomicBool);
impl ExportAdmission {
    /// Reserves one export; cancellation releases admission through the permit.
    #[must_use]
    pub fn try_start(self: &Arc<Self>) -> Option<ExportPermit> {
        // Acquire/Release pairs with permit drop before a later export is admitted.
        (!self.0.swap(true, Ordering::AcqRel)).then(|| ExportPermit(Arc::clone(self)))
    }
    /// Whether this runtime currently owns an admitted export.
    #[must_use]
    pub fn exporting(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
/// A move-only export reservation released on completion or cancellation.
pub struct ExportPermit(Arc<ExportAdmission>);
impl Drop for ExportPermit {
    fn drop(&mut self) {
        self.0.0.store(false, Ordering::Release);
    }
}

/// Projects explicit observations through the bounded support schema.
#[must_use]
pub fn report(
    driver: Option<&DriverSnapshot>,
    runtime: Option<&Value>,
    editor: Option<&EditorContext>,
    environment: &ReportEnvironment<'_>,
) -> Value {
    let devices = driver.map(|snapshot| {
        snapshot
            .settings
            .devices
            .iter()
            .take(DEVICE_LIMIT)
            .enumerate()
            .map(|(index, device)| {
                json!({
                    "index": index + 1,
                    "render": device.native_formats.render.current,
                    "capture": device.native_formats.capture.current,
                    "observedSampleRate": device.observed_sample_rate,
                    "renderChannels": device.spec.render.map(|stream| stream.channels),
                    "captureChannels": device.spec.capture.map(|stream| stream.channels),
                })
            })
            .collect::<Vec<_>>()
    });
    let driver_version = driver
        .and_then(|snapshot| snapshot.installed_version.as_deref())
        .filter(|version| version.len() <= 64)
        .and_then(|version| semver::Version::parse(version).ok())
        .map(|version| format!("{}.{}.{}", version.major, version.minor, version.patch));
    json!({
        "schemaVersion": 1,
        "product": "MixRelay Driver",
        "buildVersion": environment.build_version,
        "platform": environment.platform,
        "architecture": environment.architecture,
        "osVersion": environment.os_version,
        "createdUnixSeconds": environment.created_unix_seconds,
        "collection": "manual_snapshot",
        "driver": {
            "available": driver.is_some(),
            "version": driver_version,
            "state": driver.map(|snapshot| snapshot.state),
            "revision": driver.map(|snapshot| snapshot.revision),
            "protocolVersion": driver.map(|snapshot| snapshot.protocol_version),
            "persistenceFailed": driver.map(|snapshot| snapshot.persistence_error.is_some()),
            "deviceCount": driver.map(|snapshot| snapshot.settings.devices.len()),
            "devicesTruncated": driver.is_some_and(|snapshot| snapshot.settings.devices.len() > DEVICE_LIMIT),
            "devices": devices,
        },
        "runtime": runtime_summary(runtime),
        "editor": editor.map(EditorContext::to_report),
        "exclusions": ["audio", "memory", "logs", "credentials", "user_paths", "device_names", "device_ids", "graphs", "application_list"],
    })
}

// Copy facts through a positive schema, never redact a full arbitrary response afterwards.
fn runtime_summary(response: Option<&Value>) -> Value {
    let body = response
        .filter(|value| value["status"] == 200)
        .map(|value| &value["body"]);
    let mut summary = json!({"available": body.is_some()});
    let Some(body) = body else {
        return summary;
    };
    for key in [
        "protocolVersion",
        "sampleRate",
        "bufferFrames",
        "underruns",
        "overruns",
    ] {
        if let Some(value) = body[key].as_u64() {
            summary[key] = json!(value);
        }
    }
    let session = &body["audioSession"];
    for key in ["sourceSamples", "inputSamples", "outputSamples", "blocks"] {
        if let Some(value) = body["sampleFaults"][key].as_u64() {
            summary["sampleFaults"][key] = json!(value);
        }
    }
    for key in [
        "discontinuities",
        "observedDeviceSampleRate",
        "observedDeviceBufferFrames",
        "transportLatencyFrames",
    ] {
        if let Some(value) = session[key].as_u64() {
            summary["audioSession"][key] = json!(value);
        }
    }
    for key in ["sampleRate", "blockFrames"] {
        if let Some(value) = session["format"][key].as_u64() {
            summary["audioSession"]["format"][key] = json!(value);
        }
    }
    if let Ok(state) = serde_json::from_value::<mixrelay_audio_io::process::ProcessingState>(
        session["processingState"].clone(),
    ) {
        summary["audioSession"]["processingState"] = json!(state);
    }
    if let Ok(issue) = serde_json::from_value::<Option<mixrelay_audio_io::session::SessionIssue>>(
        session["issue"].clone(),
    ) {
        summary["audioSession"]["issue"] = json!(issue);
    }
    for key in ["enabled", "transitioning"] {
        if let Some(value) = body["enginePower"][key].as_bool() {
            summary["enginePower"][key] = json!(value);
        }
    }
    summary
}

#[cfg(test)]
mod tests;
