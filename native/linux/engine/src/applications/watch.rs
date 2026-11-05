//! Control-side record of each capture's selector, process pin and stream set; the
//! recovery worker compares it with the registry to follow replaced streams.
use super::{
    CaptureLease, CaptureSet, preparation, selection::matching_targets, selection::serials,
};
use mixrelay_audio_graph::{ApplicationCaptureRequest, RenderFormat};
use mixrelay_linux_audio_io::{
    PipeWireTarget,
    process_identity::{ProcessIdentity, process_identity},
};
use mixrelay_linux_pipewire::StreamOwners;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct WatchEntry {
    pub(super) owners: StreamOwners,
    pub(super) lease: CaptureLease,
    pub(super) node_id: String,
    pub(super) request: ApplicationCaptureRequest,
    pub(super) pinned_process: Option<ProcessIdentity>,
    pub(super) serials: Vec<String>,
    pub(super) first_pcm: Vec<(String, Arc<AtomicBool>)>,
    pub(super) healthy: Arc<AtomicBool>,
    pub(super) format: RenderFormat,
}

#[derive(Clone)]
pub(crate) struct CaptureWatch {
    pub(crate) generation: u64,
    pub(super) format: RenderFormat,
    pub(super) entries: Vec<WatchEntry>,
}

/// Current native stream whose exact capture tap has delivered pre-mute PCM.
pub(crate) struct VerifiedPlaybackTarget {
    pub(crate) object_id: u32,
    pub(crate) serial: String,
    pub(crate) process: ProcessIdentity,
    pub(crate) healthy: Arc<AtomicBool>,
    pub(crate) fresh_pcm: Arc<AtomicBool>,
}

impl CaptureWatch {
    /// Rechecks this acquired tap's current stream serial and kernel process birth.
    /// A selector match alone never grants an endpoint playback lease.
    pub(crate) fn verified_playback_targets(
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

    /// Prepares replacement taps for every capture whose stream set changed or went
    /// unhealthy. Returns the replacements with the watch that describes them.
    pub(crate) fn recover(
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
            next_entry.first_pcm = preparation::observed_streams(&targets);
            // Removing the final stream is also an update: empty capture remains
            // watched, produces silence, and reconnects when the app resumes.
            let Ok(capture) = preparation::prepare(&next_entry, &targets, stopping) else {
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
    /// The current matching streams when they differ from the acquired set or the
    /// acquired taps went unhealthy; `None` when nothing needs replacing.
    pub(super) fn changed_targets<'a>(
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
