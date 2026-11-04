//! Opening capture taps off RT: requests are validated, matched against the registry
//! and given negotiated native taps before any of them is published.
use super::{
    Capture, CaptureSet, progress::Progress, reconciliation::Preparation,
    selection::matching_targets, selection::serials, watch::CaptureWatch, watch::WatchEntry,
};
use crate::runtime::{MAX_ENDPOINT_CHANNELS, native_format, sanitize_node_name};
use mixrelay_audio_graph::{
    ApplicationCaptureRequest, ApplicationSelector, RenderFormat, SourcePlaybackPolicy,
};
use mixrelay_linux_audio_io::{PipeWireTarget, process_identity::process_identity};
use mixrelay_linux_pipewire::{NativeStream, StreamOwners, StreamRole};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
pub(super) enum CapturePreparationError {
    #[error("application capture preparation cancelled")]
    Cancelled,
    #[error("PipeWire did not negotiate application capture format")]
    FormatTimeout,
    #[error("could not reserve application capture PCM: {0}")]
    Allocation(#[from] std::collections::TryReserveError),
    #[error("application capture target has no matching PCM observation")]
    MissingPcmObservation,
    #[error("could not prepare application capture: {0}")]
    Native(String),
}

impl CaptureSet {
    #[cfg(test)]
    pub(crate) fn open(
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
    ) -> Result<(Self, CaptureWatch), String> {
        Self::open_format(
            &StreamOwners::default(),
            requests,
            generation,
            crate::runtime::render_format(),
            None,
        )
    }

    /// Prepares every requested capture, retaining unchanged ones from `previous`.
    /// The registry is read only when some request needs new taps.
    pub(crate) fn open_format(
        owners: &StreamOwners,
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
        format: RenderFormat,
        previous: Option<&CaptureWatch>,
    ) -> Result<(Self, CaptureWatch), String> {
        let preparation = Preparation::new(generation, format, previous);
        let inventory = if requests.iter().all(|(id, request)| {
            request.requires_reselection || preparation.retained(id, request).is_some()
        }) {
            Vec::new()
        } else {
            mixrelay_linux_audio_io::targets().map_err(|error| error.to_string())?
        };
        Self::open_inventory_format(owners, requests, &inventory, &preparation)
    }

    #[cfg(test)]
    pub(super) fn open_inventory(
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
        inventory: &[PipeWireTarget],
    ) -> Result<(Self, CaptureWatch), String> {
        Self::open_inventory_format(
            &StreamOwners::default(),
            requests,
            inventory,
            &Preparation::new(generation, crate::runtime::render_format(), None),
        )
    }

    pub(super) fn open_inventory_format(
        owners: &StreamOwners,
        requests: &[(String, ApplicationCaptureRequest)],
        inventory: &[PipeWireTarget],
        preparation: &Preparation<'_>,
    ) -> Result<(Self, CaptureWatch), String> {
        let mut set = Self {
            generation: preparation.epoch,
            ..Self::default()
        };
        let mut watch = CaptureWatch {
            generation: preparation.epoch,
            format: preparation.format,
            entries: Vec::with_capacity(requests.len()),
        };
        for (node_id, request) in requests {
            request
                .validate_contract(MAX_ENDPOINT_CHANNELS)
                .map_err(|error| error.to_string())?;
            if request.source_playback == SourcePlaybackPolicy::MuteWhileCaptured {
                return Err("PipeWire source muting is not implemented".into());
            }
            if let Some(entry) = preparation.retained(node_id, request) {
                set.nodes
                    .insert(node_id.clone(), Capture::placeholder(entry));
                set.retained.insert(node_id.clone(), entry.lease);
                watch.entries.push(entry.clone());
                continue;
            }
            let pin = pin_process(node_id, request)?;
            let targets = if request.requires_reselection {
                Vec::new()
            } else {
                matching_targets(inventory, &request.selector, pin.as_ref(), process_identity)
            };
            let entry = WatchEntry {
                owners: owners.clone(),
                lease: preparation.lease,
                node_id: node_id.clone(),
                request: request.clone(),
                pinned_process: pin,
                serials: serials(&targets),
                first_pcm: observed_streams(&targets),
                healthy: Arc::new(AtomicBool::new(true)),
                format: preparation.format,
            };
            let capture = prepare(&entry, &targets, &AtomicBool::new(false))
                .map_err(|error| error.to_string())?;
            watch.entries.push(entry);
            set.nodes.insert(node_id.clone(), capture);
        }
        Ok((set, watch))
    }
}

/// A process selector is pinned to the kernel's identity of that process at selection
/// time, so a recycled PID never inherits the capture.
fn pin_process(
    node_id: &str,
    request: &ApplicationCaptureRequest,
) -> Result<Option<mixrelay_linux_audio_io::process_identity::ProcessIdentity>, String> {
    match request.selector {
        _ if request.requires_reselection => Ok(None),
        ApplicationSelector::ProcessId { process_id, .. } => process_identity(process_id)
            .map(Some)
            .ok_or_else(|| format!("cannot verify selected process identity for {node_id}")),
        _ => Ok(None),
    }
}

/// One unset first-PCM cell per stream; the tap sets it once real PCM arrives.
pub(super) fn observed_streams(targets: &[&PipeWireTarget]) -> Vec<(String, Arc<AtomicBool>)> {
    targets
        .iter()
        .map(|target| (target.serial.clone(), Arc::new(AtomicBool::new(false))))
        .collect()
}

pub(super) fn prepare(
    entry: &WatchEntry,
    targets: &[&PipeWireTarget],
    stopping: &AtomicBool,
) -> Result<Capture, CapturePreparationError> {
    if stopping.load(Ordering::Acquire) {
        return Err(CapturePreparationError::Cancelled);
    }
    let scratch = mixrelay_audio_io::pcm_buffer::zeroed(
        usize::from(entry.request.stream.channels) * entry.format.block_frames as usize,
    )?
    .into_boxed_slice();
    let first_pcm = targets
        .iter()
        .map(|target| {
            entry
                .first_pcm
                .iter()
                .find(|(serial, _)| serial == &target.serial)
                .map(|(_, cell)| Arc::clone(cell))
                .ok_or(CapturePreparationError::MissingPcmObservation)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let taps = targets
        .iter()
        .map(|target| {
            if stopping.load(Ordering::Acquire) {
                return Err(CapturePreparationError::Cancelled);
            }
            NativeStream::open_format(
                &entry.owners,
                &format!(
                    "{}.capture.{}",
                    sanitize_node_name(&entry.node_id),
                    target.serial
                ),
                &target.display_name,
                StreamRole::ApplicationCapture(&target.serial),
                entry.request.stream.channels,
                native_format(entry.format),
            )
            .map_err(CapturePreparationError::Native)
        })
        .collect::<Result<Vec<_>, _>>()?;
    wait_for_capture_format(&taps, stopping)?;
    Ok(Capture {
        lease: entry.lease,
        taps,
        progress: first_pcm
            .iter()
            .map(|_| Progress::new(entry.format.sample_rate))
            .collect(),
        first_pcm,
        healthy: entry.healthy.clone(),
        scratch,
    })
}

/// Native negotiation must finish before RT health checks can treat a stream as
/// failed. Otherwise a fast recovery loop repeatedly replaces its own pending tap.
/// Dropping a failed/cancelled preparation drains only these unpublished owners.
pub(super) fn wait_for_capture_format(
    taps: &[NativeStream],
    stopping: &AtomicBool,
) -> Result<(), CapturePreparationError> {
    let started = Instant::now();
    loop {
        if stopping.load(Ordering::Acquire) {
            return Err(CapturePreparationError::Cancelled);
        }
        if taps.iter().all(NativeStream::ready) {
            return Ok(());
        }
        if started.elapsed() >= Duration::from_secs(3) {
            return Err(CapturePreparationError::FormatTimeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
}
