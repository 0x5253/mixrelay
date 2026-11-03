//! Native source transfer at a graph boundary; control metadata never owns PCM cursors.
use super::{Capture, CaptureLease, CaptureSet, CaptureWatch, WatchEntry};
use mixrelay_audio_graph::{ApplicationCaptureRequest, RenderFormat};
use std::collections::BTreeMap;

/// Control-only edit context. The old watcher lends immutable lease metadata,
/// never native handles; a changed format starts a new recovery epoch.
pub(super) struct Preparation<'a> {
    pub epoch: u64,
    pub lease: CaptureLease,
    pub format: RenderFormat,
    previous: BTreeMap<&'a str, &'a WatchEntry>,
}

impl<'a> Preparation<'a> {
    pub(super) fn new(
        generation: u64,
        format: RenderFormat,
        previous: Option<&'a CaptureWatch>,
    ) -> Self {
        let previous = previous.filter(|watch| watch.format == format);
        Self {
            epoch: previous.map_or(generation, |watch| watch.generation),
            lease: CaptureLease(generation),
            format,
            previous: previous.map(CaptureWatch::by_node).unwrap_or_default(),
        }
    }

    pub(super) fn retained(
        &self,
        id: &str,
        request: &ApplicationCaptureRequest,
    ) -> Option<&WatchEntry> {
        self.previous
            .get(id)
            .copied()
            .filter(|entry| entry.request == *request)
    }
}

impl Capture {
    // Reserve the exact map slot off RT. This owns no stream, queued samples or
    // scratch allocation; activation swaps the retained owner into this slot.
    pub(super) fn placeholder(entry: &WatchEntry) -> Self {
        Self {
            lease: entry.lease,
            taps: Vec::new(),
            first_pcm: Vec::new(),
            progress: Vec::new(),
            scratch: Box::default(),
            healthy: entry.healthy.clone(),
        }
    }
}

impl CaptureWatch {
    pub(super) fn by_node(&self) -> BTreeMap<&str, &WatchEntry> {
        self.entries
            .iter()
            .map(|entry| (entry.node_id.as_str(), entry))
            .collect()
    }

    /// Refreshes only the same source lease under the backend's watcher lock.
    /// A recovery finishing during an edit cannot erase new nodes, restore removed
    /// nodes or replace a newly selected source's metadata with an old snapshot.
    pub(crate) fn refresh_leases(&mut self, source: &Self) {
        if self.generation != source.generation {
            return;
        }
        let source = source.by_node();
        for entry in &mut self.entries {
            if let Some(updated) = source.get(entry.node_id.as_str())
                && entry.lease == updated.lease
            {
                entry.clone_from(updated);
            }
        }
    }
}

impl CaptureSet {
    /// Validates every retained slot before any part of a native transaction moves.
    /// Uses only bounded lookups; it does not allocate, mutate or destroy owners.
    pub(crate) fn can_retain_from(&self, current: &Self) -> bool {
        self.retained.is_empty()
            || (self.generation == current.generation
                && self.retained.iter().all(|(id, lease)| {
                    current
                        .nodes
                        .get(id)
                        .is_some_and(|owner| owner.lease == *lease)
                        && self.nodes.get(id).is_some_and(|slot| slot.lease == *lease)
                }))
    }

    /// Called only after transaction-wide validation, with exclusive audio ownership.
    /// Retains cursor, queued PCM, health cell and process pin. The displaced empty
    /// slots and their map storage stay in the retired envelope for off-RT disposal.
    pub(crate) fn retain_from(&mut self, current: &mut Self) {
        self.discard_inputs(); // Only freshly opened taps exist here before transfer.
        for id in self.retained.keys() {
            if let (Some(slot), Some(owner)) = (self.nodes.get_mut(id), current.nodes.get_mut(id)) {
                std::mem::swap(slot, owner);
            }
        }
    }
}

#[cfg(test)]
#[path = "linux_capture_reconciliation_tests.rs"]
mod tests;
