//! Application intent survives registry stream replacement. Discovery and native
//! ownership changes run off RT; callback activation only swaps existing slots.
use super::{MAX_ENDPOINT_CHANNELS, native::NativeStream, sanitize_node_name};
use mixrelay_audio_graph::{ApplicationCaptureRequest, ApplicationSelector, SourcePlaybackPolicy};
use mixrelay_audio_io::process::{InputActivity, ProcessReport, TransferOutcome};
use mixrelay_linux_audio_io::{
    PipeWireTarget,
    process_identity::{ProcessIdentity, process_identity, same_process},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[path = "linux_application_progress.rs"]
mod progress;

#[derive(Default)]
pub(super) struct CaptureSet {
    // Format epoch fences wholesale format replacement. Per-source leases fence
    // removed/re-added nodes without invalidating other sources' pending recovery.
    pub generation: u64,
    nodes: BTreeMap<String, Capture>,
    retained: BTreeMap<String, CaptureLease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureLease(u64);

struct Capture {
    lease: CaptureLease,
    taps: Vec<NativeStream>,
    first_pcm: Vec<Arc<AtomicBool>>,
    progress: Vec<progress::Progress>,
    scratch: Box<[f32]>,
    healthy: Arc<AtomicBool>,
}

#[derive(Debug, thiserror::Error)]
enum CapturePreparationError {
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

#[derive(Clone)]
struct WatchEntry {
    owners: super::native::StreamOwners,
    lease: CaptureLease,
    node_id: String,
    request: ApplicationCaptureRequest,
    pinned_process: Option<ProcessIdentity>,
    serials: Vec<String>,
    first_pcm: Vec<(String, Arc<AtomicBool>)>,
    healthy: Arc<AtomicBool>,
    format: mixrelay_audio_graph::RenderFormat,
}

#[derive(Clone)]
pub(super) struct CaptureWatch {
    pub generation: u64,
    format: mixrelay_audio_graph::RenderFormat,
    entries: Vec<WatchEntry>,
}

/// Current native stream whose exact capture tap has delivered pre-mute PCM.
pub(super) struct VerifiedPlaybackTarget {
    pub object_id: u32,
    pub serial: String,
    pub process: ProcessIdentity,
    pub healthy: Arc<AtomicBool>,
    pub fresh_pcm: Arc<AtomicBool>,
}

impl CaptureSet {
    pub(super) fn bind_inputs(
        &mut self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), String> {
        for capture in self.nodes.values_mut() {
            for tap in &mut capture.taps {
                tap.bind_inputs(inputs)?;
            }
        }
        Ok(())
    }
    pub(super) fn discard_inputs(&mut self) {
        for capture in self.nodes.values_mut() {
            for tap in &mut capture.taps {
                tap.discard_input();
            }
        }
    }
    pub(super) fn finish_block(&mut self, frames: u32) {
        for capture in self.nodes.values_mut() {
            for (index, tap) in capture.taps.iter_mut().enumerate() {
                let used = tap.finish_capture_block();
                let live = capture.progress[index].finish(frames, used);
                if !live && let Some(first_pcm) = capture.first_pcm.get(index) {
                    first_pcm.store(false, Ordering::Release);
                }
            }
        }
    }
    pub(super) fn formats_ready(&self) -> bool {
        self.nodes
            .values()
            .all(|capture| capture.taps.iter().all(NativeStream::ready))
    }
    pub(super) fn shares_clock(&self, id: u32, rate: u32) -> bool {
        self.nodes.values().all(|capture| {
            !capture.taps.is_empty() && capture.taps.iter().all(|tap| tap.shares_clock(id, rate))
        })
    }
    pub(super) fn contains(&self, node_id: &str) -> bool {
        self.nodes.contains_key(node_id)
    }
    pub(super) fn state(&self) -> mixrelay_engine_core::session::ApplicationRuntimeState {
        use mixrelay_engine_core::session::ApplicationRuntimeState;
        if self
            .nodes
            .values()
            .any(|capture| capture.taps.is_empty() || !capture.taps.iter().all(NativeStream::ready))
        {
            ApplicationRuntimeState::Waiting
        } else {
            ApplicationRuntimeState::Ready
        }
    }
    #[cfg(test)]
    pub(super) fn open(
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
    ) -> Result<(Self, CaptureWatch), String> {
        Self::open_format(
            &super::native::StreamOwners::default(),
            requests,
            generation,
            super::render_format(),
            None,
        )
    }

    pub(super) fn open_format(
        owners: &super::native::StreamOwners,
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
        format: mixrelay_audio_graph::RenderFormat,
        previous: Option<&CaptureWatch>,
    ) -> Result<(Self, CaptureWatch), String> {
        let preparation = reconciliation::Preparation::new(generation, format, previous);
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
    fn open_inventory(
        requests: &[(String, ApplicationCaptureRequest)],
        generation: u64,
        inventory: &[PipeWireTarget],
    ) -> Result<(Self, CaptureWatch), String> {
        Self::open_inventory_format(
            &super::native::StreamOwners::default(),
            requests,
            inventory,
            &reconciliation::Preparation::new(generation, super::render_format(), None),
        )
    }

    fn open_inventory_format(
        owners: &super::native::StreamOwners,
        requests: &[(String, ApplicationCaptureRequest)],
        inventory: &[PipeWireTarget],
        preparation: &reconciliation::Preparation<'_>,
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
            let pin = match request.selector {
                _ if request.requires_reselection => None,
                ApplicationSelector::ProcessId { process_id, .. } => {
                    Some(process_identity(process_id).ok_or_else(|| {
                        format!("cannot verify selected process identity for {node_id}")
                    })?)
                }
                _ => None,
            };
            let targets = if request.requires_reselection {
                Vec::new()
            } else {
                matching_targets(inventory, &request.selector, pin.as_ref(), process_identity)
            };
            let healthy = Arc::new(AtomicBool::new(true));
            let entry = WatchEntry {
                owners: owners.clone(),
                lease: preparation.lease,
                node_id: node_id.clone(),
                request: request.clone(),
                pinned_process: pin,
                serials: serials(&targets),
                first_pcm: observed_streams(&targets),
                healthy,
                format: preparation.format,
            };
            let capture = prepare(&entry, &targets, &AtomicBool::new(false))
                .map_err(|error| error.to_string())?;
            watch.entries.push(entry);
            set.nodes.insert(node_id.clone(), capture);
        }
        Ok((set, watch))
    }

    pub(super) fn replace(&mut self, next: &mut Self) {
        for (node, capture) in &mut next.nodes {
            if let Some(current) = self.nodes.get_mut(node)
                && current.lease == capture.lease
            {
                for tap in &mut capture.taps {
                    tap.discard_input();
                }
                std::mem::swap(current, capture);
            }
        }
    }

    /// Reads no more than the prepared PCM quantum. An oversized request is cleared and
    /// rejected without consuming native PCM or resizing callback storage.
    pub(super) fn read(&mut self, node: &str, destination: &mut [f32]) -> TransferOutcome {
        destination.fill(0.0);
        let Some(capture) = self.nodes.get_mut(node) else {
            return TransferOutcome {
                activity: InputActivity::Unknown,
                ..TransferOutcome::complete(0)
            };
        };
        let Some(scratch) = capture.scratch.get_mut(..destination.len()) else {
            return TransferOutcome {
                frames: 0,
                activity: InputActivity::Unknown,
                discontinuity: true,
                source_sample_faults: 0,
            };
        };
        capture.healthy.store(
            capture.taps.iter().all(NativeStream::ready),
            Ordering::Release,
        );
        let mut transfer = TransferOutcome::complete(0);
        transfer.activity =
            if capture.taps.is_empty() || !capture.taps.iter().all(NativeStream::ready) {
                InputActivity::Unknown
            } else {
                InputActivity::Inactive
            };
        for (index, tap) in capture.taps.iter_mut().enumerate() {
            scratch.fill(0.0);
            let activity = tap.input_activity();
            if activity != InputActivity::Active {
                tap.discard_input();
                // PAUSED is explicit native inactivity, not an empty-ring guess.
                transfer.discontinuity |= activity == InputActivity::Unknown;
                if activity == InputActivity::Unknown && transfer.activity != InputActivity::Active
                {
                    transfer.activity = InputActivity::Unknown;
                }
                continue;
            }
            transfer.activity = InputActivity::Active;
            let outcome = tap.read_capture(scratch);
            let frames = outcome.frames;
            capture.progress[index].delivered(frames);
            if frames > 0
                && let Some(first_pcm) = capture.first_pcm.get(index)
            {
                first_pcm.store(true, Ordering::Release);
            }
            transfer.frames = transfer.frames.max(frames);
            transfer.discontinuity |= outcome.discontinuity || frames == 0;
            transfer.source_sample_faults = transfer
                .source_sample_faults
                .saturating_add(outcome.source_sample_faults);
            for (mixed, sample) in destination.iter_mut().zip(scratch.iter()) {
                *mixed += sample;
            }
        }
        transfer
    }

    pub(super) fn take_report(&mut self) -> ProcessReport {
        let mut report = ProcessReport::default();
        for capture in self.nodes.values_mut() {
            capture.healthy.store(
                capture.taps.iter().all(NativeStream::ready),
                Ordering::Release,
            );
            for tap in &mut capture.taps {
                let next = tap.take_report();
                report.input_starvations += next.input_starvations;
                report.output_overflows += next.output_overflows;
                report.discontinuities += next.discontinuities;
            }
        }
        report
    }
}

#[cfg(test)]
impl CaptureSet {
    /// Observes current canonical capture owners without changing their report cursors.
    #[cfg(feature = "native-lab")]
    pub(super) fn transfer_snapshot(&self) -> super::native::TransferSnapshot {
        let mut snapshot = super::native::TransferSnapshot::default();
        for stream in self.nodes.values().flat_map(|capture| &capture.taps) {
            snapshot.include(stream);
        }
        snapshot
    }
}

impl CaptureWatch {
    /// Rechecks this acquired tap's current stream serial and kernel process birth.
    /// A selector match alone never grants an endpoint playback lease.
    pub(super) fn verified_playback_targets(
        &self,
        node_id: &str,
        inventory: &[PipeWireTarget],
    ) -> Vec<VerifiedPlaybackTarget> {
        let Some(entry) = self.entries.iter().find(|entry| entry.node_id == node_id) else {
            return Vec::new();
        };
        if entry.request.requires_reselection || !entry.healthy.load(Ordering::Acquire) {
            return Vec::new();
        }
        let targets = matching_targets(
            inventory,
            &entry.request.selector,
            entry.pinned_process.as_ref(),
            process_identity,
        );
        if targets.is_empty() || serials(&targets) != entry.serials {
            return Vec::new();
        }
        targets
            .into_iter()
            .map(|target| {
                let observed = entry
                    .first_pcm
                    .iter()
                    .find(|(serial, _)| serial == &target.serial)?;
                if !observed.1.load(Ordering::Acquire) {
                    return None;
                }
                let process = process_identity(target.process_id?)?;
                Some(VerifiedPlaybackTarget {
                    object_id: target.object_id,
                    serial: target.serial.clone(),
                    process,
                    healthy: Arc::clone(&entry.healthy),
                    fresh_pcm: Arc::clone(&observed.1),
                })
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
    }

    pub(super) fn recover(
        &self,
        stopping: &AtomicBool,
    ) -> Result<Option<(CaptureSet, Self)>, String> {
        if self
            .entries
            .iter()
            .all(|entry| entry.request.requires_reselection)
            || stopping.load(Ordering::Acquire)
        {
            return Ok(None);
        }
        let inventory = mixrelay_linux_audio_io::targets().map_err(|error| error.to_string())?;
        let mut updates = CaptureSet {
            generation: self.generation,
            ..CaptureSet::default()
        };
        let mut next_watch = self.clone();
        for entry in &mut next_watch.entries {
            if stopping.load(Ordering::Acquire) {
                return Ok(None);
            }
            let Some(targets) = entry.changed_targets(&inventory, process_identity) else {
                continue;
            };
            let next_serials = serials(&targets);
            let mut next_entry = entry.clone();
            next_entry.first_pcm = observed_streams(&targets);
            // Removing the final stream is also an update: empty capture remains
            // watched, produces silence, and reconnects when the app resumes.
            let Ok(capture) = prepare(&next_entry, &targets, stopping) else {
                continue;
            };
            entry.serials = next_serials;
            entry.first_pcm = next_entry.first_pcm;
            updates.nodes.insert(entry.node_id.clone(), capture);
        }
        Ok((!updates.nodes.is_empty()).then_some((updates, next_watch)))
    }
}

impl WatchEntry {
    fn changed_targets<'a>(
        &self,
        inventory: &'a [PipeWireTarget],
        lookup: impl Fn(u32) -> Option<ProcessIdentity>,
    ) -> Option<Vec<&'a PipeWireTarget>> {
        if self.request.requires_reselection {
            return None;
        }
        let targets = matching_targets(
            inventory,
            &self.request.selector,
            self.pinned_process.as_ref(),
            lookup,
        );
        (serials(&targets) != self.serials || !self.healthy.load(Ordering::Acquire))
            .then_some(targets)
    }
}

fn serials(targets: &[&PipeWireTarget]) -> Vec<String> {
    let mut serials = targets
        .iter()
        .map(|target| target.serial.clone())
        .collect::<Vec<_>>();
    serials.sort_unstable();
    serials.dedup();
    serials
}

fn observed_streams(targets: &[&PipeWireTarget]) -> Vec<(String, Arc<AtomicBool>)> {
    targets
        .iter()
        .map(|target| (target.serial.clone(), Arc::new(AtomicBool::new(false))))
        .collect()
}

fn prepare(
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
                super::native::StreamRole::ApplicationCapture(&target.serial),
                entry.request.stream.channels,
                super::native_format(entry.format),
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
            .map(|_| progress::Progress::new(entry.format.sample_rate))
            .collect(),
        first_pcm,
        healthy: entry.healthy.clone(),
        scratch,
    })
}

#[path = "linux_capture_reconciliation.rs"]
mod reconciliation;

/// Native negotiation must finish before RT health checks can treat a stream as
/// failed. Otherwise a fast recovery loop repeatedly replaces its own pending tap.
/// Dropping a failed/cancelled preparation drains only these unpublished owners.
fn wait_for_capture_format(
    taps: &[NativeStream],
    stopping: &AtomicBool,
) -> Result<(), CapturePreparationError> {
    let started = std::time::Instant::now();
    loop {
        if stopping.load(Ordering::Acquire) {
            return Err(CapturePreparationError::Cancelled);
        }
        if taps.iter().all(NativeStream::ready) {
            return Ok(());
        }
        if started.elapsed() >= std::time::Duration::from_secs(3) {
            return Err(CapturePreparationError::FormatTimeout);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn matching_targets<'a>(
    inventory: &'a [PipeWireTarget],
    selector: &ApplicationSelector,
    pin: Option<&ProcessIdentity>,
    lookup: impl Fn(u32) -> Option<ProcessIdentity>,
) -> Vec<&'a PipeWireTarget> {
    let pinned_alive = pin.is_some_and(|pin| {
        lookup(pin.process_id).is_some_and(|current| same_process(pin, &current))
    });
    let matches = inventory
        .iter()
        .filter(|target| {
            target.media_class == "Stream/Output/Audio"
                && target.process_id != Some(std::process::id())
        })
        .filter(|target| match selector {
            ApplicationSelector::BundleId { bundle_id, .. } => {
                target.application_id.as_deref() == Some(bundle_id)
            }
            ApplicationSelector::ExecutablePath { path } => target
                .process_id
                .and_then(&lookup)
                .and_then(|process| process.executable_path)
                .is_some_and(|actual| {
                    actual == *path
                        && target.executable.as_ref().is_some_and(|reported| {
                            reported == &actual
                                || std::path::Path::new(&actual)
                                    .file_name()
                                    .and_then(|name| name.to_str())
                                    == Some(reported)
                        })
                }),
            ApplicationSelector::ProcessId {
                process_id,
                include_descendants,
            } => {
                pinned_alive
                    && target.process_id.is_some_and(|pid| {
                        pid == *process_id
                            || (*include_descendants && descendant(pid, *process_id, &lookup))
                    })
            }
            ApplicationSelector::AllExcept { bundle_ids } => !target
                .application_id
                .as_ref()
                .or(target.executable.as_ref())
                .is_some_and(|id| bundle_ids.contains(id)),
        })
        .collect();
    // Recheck after ancestry walking: an exit/reused root during discovery may
    // not transfer the legacy process intent to a different process family.
    if pin.is_some_and(|pin| {
        lookup(pin.process_id).is_none_or(|current| !same_process(pin, &current))
    }) {
        Vec::new()
    } else {
        matches
    }
}

fn descendant(
    mut candidate: u32,
    ancestor: u32,
    lookup: &impl Fn(u32) -> Option<ProcessIdentity>,
) -> bool {
    for _ in 0..64 {
        let Some(identity) = lookup(candidate) else {
            return false;
        };
        if identity.parent_id == ancestor {
            return true;
        }
        if identity.parent_id <= 1 || identity.parent_id == candidate {
            return false;
        }
        candidate = identity.parent_id;
    }
    false
}

#[cfg(test)]
#[path = "linux_application_tests.rs"]
pub(crate) mod tests;
