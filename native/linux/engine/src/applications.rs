//! Application intent survives registry stream replacement. Discovery and native
//! ownership changes run off RT; callback activation only swaps existing slots.
use mixrelay_audio_io::process::{InputActivity, ProcessReport, TransferOutcome};
use mixrelay_engine_core::session::ApplicationRuntimeState;
use mixrelay_linux_pipewire::{GraphInputs, NativeStream};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) mod identity;
mod preparation;
mod progress;
mod reconciliation;
mod selection;
#[cfg(test)]
pub(crate) mod tests;
mod watch;

pub(crate) use watch::CaptureWatch;

#[derive(Default)]
pub(crate) struct CaptureSet {
    // Format epoch fences wholesale format replacement. Per-source leases fence
    // removed/re-added nodes without invalidating other sources' pending recovery.
    pub(crate) generation: u64,
    nodes: BTreeMap<String, Capture>,
    retained: BTreeMap<String, CaptureLease>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaptureLease(u64);

/// Native taps for one capture node; one tap per matching application stream.
struct Capture {
    lease: CaptureLease,
    taps: Vec<NativeStream>,
    first_pcm: Vec<Arc<AtomicBool>>,
    progress: Vec<progress::Progress>,
    scratch: Box<[f32]>,
    healthy: Arc<AtomicBool>,
}

impl Capture {
    fn taps_ready(&self) -> bool {
        self.taps.iter().all(NativeStream::ready)
    }
}

impl CaptureSet {
    pub(crate) fn bind_inputs(&mut self, inputs: &GraphInputs) -> Result<(), String> {
        for tap in self.taps_mut() {
            tap.bind_inputs(inputs)?;
        }
        Ok(())
    }

    pub(crate) fn discard_inputs(&mut self) {
        for tap in self.taps_mut() {
            tap.discard_input();
        }
    }

    fn taps_mut(&mut self) -> impl Iterator<Item = &mut NativeStream> {
        self.nodes
            .values_mut()
            .flat_map(|capture| capture.taps.iter_mut())
    }

    pub(crate) fn finish_block(&mut self, frames: u32) {
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

    pub(crate) fn formats_ready(&self) -> bool {
        self.nodes.values().all(Capture::taps_ready)
    }

    pub(crate) fn shares_clock(&self, id: u32, rate: u32) -> bool {
        self.nodes.values().all(|capture| {
            !capture.taps.is_empty() && capture.taps.iter().all(|tap| tap.shares_clock(id, rate))
        })
    }

    pub(crate) fn contains(&self, node_id: &str) -> bool {
        self.nodes.contains_key(node_id)
    }

    /// Waiting while any node has no stream or a stream still negotiating.
    pub(crate) fn state(&self) -> ApplicationRuntimeState {
        if self
            .nodes
            .values()
            .any(|capture| capture.taps.is_empty() || !capture.taps_ready())
        {
            ApplicationRuntimeState::Waiting
        } else {
            ApplicationRuntimeState::Ready
        }
    }

    /// RT: swaps recovered taps into nodes still holding the same source lease.
    pub(crate) fn replace(&mut self, next: &mut Self) {
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
    pub(crate) fn read(&mut self, node: &str, destination: &mut [f32]) -> TransferOutcome {
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
        let ready = capture.taps.iter().all(NativeStream::ready);
        capture.healthy.store(ready, Ordering::Release);
        let mut transfer = TransferOutcome::complete(0);
        transfer.activity = if capture.taps.is_empty() || !ready {
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

    pub(crate) fn take_report(&mut self) -> ProcessReport {
        let mut report = ProcessReport::default();
        for capture in self.nodes.values_mut() {
            capture
                .healthy
                .store(capture.taps_ready(), Ordering::Release);
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

#[cfg(all(test, feature = "native-lab"))]
impl CaptureSet {
    /// Observes current canonical capture owners without changing their report cursors.
    pub(crate) fn transfer_snapshot(&self) -> mixrelay_linux_pipewire::TransferSnapshot {
        let mut snapshot = mixrelay_linux_pipewire::TransferSnapshot::default();
        for stream in self.nodes.values().flat_map(|capture| &capture.taps) {
            snapshot.include(stream);
        }
        snapshot
    }
}
