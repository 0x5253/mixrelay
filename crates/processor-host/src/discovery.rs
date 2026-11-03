//! Failure-aware discovery. Only successful stages replace the last usable catalog entries.
//! The catalog snapshot is published atomically after a serialized scan, never held locked while
//! third-party child processes run. Discovery is control-plane work, not render-thread work.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    time::Duration,
};

use mixrelay_control_protocol::{ExternalProcessorDiagnostic, ProcessorDiscoveryStage};

use crate::{
    ProcessorCatalog, RackFormat, RackLocator,
    catalog::{CatalogEntry, RackCatalogTarget},
    rack_formats,
};

mod capture;
#[cfg(test)]
mod tests;

const SCAN_TIMEOUT: Duration = Duration::from_secs(45);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub(super) struct CatalogSnapshot {
    pub entries: BTreeMap<String, CatalogEntry>,
    pub diagnostics: Vec<ExternalProcessorDiagnostic>,
}

pub(super) fn rescan(catalog: &ProcessorCatalog, custom_paths: &[PathBuf]) {
    rescan_with(
        catalog,
        &rack_formats::supported_formats(),
        |format| scan_format(format, custom_paths),
        probe_candidate,
    );
}

fn rescan_with(
    catalog: &ProcessorCatalog,
    formats: &[RackFormat],
    mut scan: impl FnMut(RackFormat) -> Result<rack_formats::RackScanReport, String>,
    mut probe: impl FnMut(&RackLocator) -> Result<rack_formats::RackProbeRecord, String>,
) {
    let _scan = catalog
        .scan_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = catalog
        .snapshot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut next = CatalogSnapshot::default();
    for &format in formats {
        match scan(format) {
            Ok(report) => {
                retain_failed_path_entries(&previous, &mut next, format, &report.path_failures);
                for failure in report.path_failures {
                    next.diagnostics.push(ExternalProcessorDiagnostic {
                        format: format.protocol(),
                        stage: ProcessorDiscoveryStage::Scan,
                        plugin_id: None,
                        plugin_name: None,
                        message: format!(
                            "custom scan path {}: {}",
                            failure.path.display(),
                            failure.message
                        ),
                    });
                }
                let mut seen = BTreeSet::new();
                for locator in report.locators {
                    if !locator.is_audio_processor() {
                        continue;
                    }
                    if seen.insert((
                        locator.format.argument(),
                        locator.unique_id.clone(),
                        locator.path.clone(),
                    )) {
                        probe_into_snapshot(&previous, &mut next, &locator, &mut probe);
                    }
                }
            }
            Err(message) => {
                next.diagnostics.push(ExternalProcessorDiagnostic {
                    format: format.protocol(),
                    stage: ProcessorDiscoveryStage::Scan,
                    plugin_id: None,
                    plugin_name: None,
                    message,
                });
                next.entries.extend(
                    previous
                        .entries
                        .iter()
                        .filter(|(_, entry)| entry.rack.locator.format == format)
                        .map(|(id, entry)| (id.clone(), entry.clone())),
                );
            }
        }
    }
    *catalog
        .snapshot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
}

fn retain_failed_path_entries(
    previous: &CatalogSnapshot,
    next: &mut CatalogSnapshot,
    format: RackFormat,
    failures: &[rack_formats::ScanPathFailure],
) {
    for (id, entry) in previous.entries.iter().filter(|(_, entry)| {
        entry.rack.locator.format == format
            && failures
                .iter()
                .any(|failure| entry.rack.locator.path.starts_with(&failure.path))
    }) {
        next.entries
            .entry(id.clone())
            .or_insert_with(|| entry.clone());
    }
}

fn probe_into_snapshot(
    previous: &CatalogSnapshot,
    next: &mut CatalogSnapshot,
    locator: &RackLocator,
    probe: &mut impl FnMut(&RackLocator) -> Result<rack_formats::RackProbeRecord, String>,
) {
    match probe(locator) {
        Ok(record) => {
            // This catalog is audio effects only. A successful non-effect result is not a
            // scanner failure and does not justify preserving a previous effect descriptor.
            if record.descriptor.layouts.is_empty() {
                next.entries.remove(&record.descriptor.type_id);
                return;
            }
            next.entries.insert(
                record.descriptor.type_id.clone(),
                CatalogEntry {
                    descriptor: record.descriptor,
                    rack: RackCatalogTarget {
                        locator: record.locator,
                    },
                },
            );
        }
        Err(message) => {
            next.diagnostics.push(ExternalProcessorDiagnostic {
                format: locator.format.protocol(),
                stage: ProcessorDiscoveryStage::Probe,
                plugin_id: Some(format!(
                    "{}:{}",
                    locator.format.argument(),
                    locator.unique_id
                )),
                plugin_name: Some(locator.display_name().to_owned()),
                message,
            });
            // A successful format scan is authoritative for removed plug-ins. Retain only the
            // exact failing candidate, not every old entry in its format or a same-ID other path.
            next.entries.extend(
                previous
                    .entries
                    .iter()
                    .filter(|(_, entry)| same_candidate(&entry.rack.locator, locator))
                    .map(|(id, entry)| (id.clone(), entry.clone())),
            );
        }
    }
}

fn same_candidate(left: &RackLocator, right: &RackLocator) -> bool {
    left.format == right.format && left.unique_id == right.unique_id && left.path == right.path
}

fn scan_format(
    format: RackFormat,
    custom_paths: &[PathBuf],
) -> Result<rack_formats::RackScanReport, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("scanner executable: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg(crate::PLUGIN_SCAN_ARGUMENT)
        .arg(format.argument())
        .args(custom_paths);
    if format == RackFormat::Lv2
        && let Some(path) = rack_formats::lv2_search_path(custom_paths)
    {
        command.env("LV2_PATH", path);
    }
    capture::capture_json_child(command, SCAN_TIMEOUT)
}

fn probe_candidate(locator: &RackLocator) -> Result<rack_formats::RackProbeRecord, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("probe executable: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg(crate::PLUGIN_PROBE_ARGUMENT)
        .arg(locator.encode()?);
    capture::capture_json_child(command, PROBE_TIMEOUT)
}
