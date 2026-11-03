//! Consumer-owned acquired-PCM age and graph readiness for application sources.
use super::{Source, SourceRegistry};
use mixrelay_audio_io::residence::{Residence, ResidenceSnapshot};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The graph's acquisition cursor and common transfer label. Missing labels or
/// coordinates provide no latency evidence, even when PCM itself can be read.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadTiming {
    pub rate: u32,
    pub first_nanos: Option<u64>,
    pub host_ticks: Option<u64>,
}

#[derive(Default)]
pub(super) struct Measurements {
    pub(super) age: Residence,
    faults: AtomicU64,
    revision: AtomicU64,
    graph_epoch: AtomicU64,
    ready: AtomicBool,
}

pub(super) struct Attempt<'a> {
    measurements: &'a Measurements,
    pub(super) completed: bool,
}
impl<'a> Attempt<'a> {
    pub(super) fn new(measurements: &'a Measurements) -> Self {
        Self {
            measurements,
            completed: false,
        }
    }
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.completed {
            // Release pairs with the reporter's Acquire; the consumer is the sole writer.
            self.measurements.faults.fetch_add(1, Ordering::Release);
        }
    }
}

/// Independently retained context, including unacknowledged format changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub lease: u64,
    pub epoch: u64,
    pub format: u64,
    pub acknowledged: u64,
    pub native_rate: u32,
    pub running: bool,
    pub revision: u64,
    pub graph_epoch: u64,
    pub ready: bool,
}
impl Context {
    fn read(source: &Source) -> Self {
        let clock = source.mapping.clock();
        let taps = &source.measurements;
        // Acquire pairs with the producer/graph's Release publications. The
        // reporter checks this context on both sides of the cumulative taps.
        Self {
            lease: clock.producer_lease.load(Ordering::Acquire),
            // SAFETY: the registry read pin retains this initialized mapping.
            epoch: unsafe { super::mr_shared_audio_buffer_stats(source.mapping.buffer.as_ptr()) }
                .epoch,
            format: clock.format_word.load(Ordering::Acquire),
            acknowledged: clock.acknowledged_format.load(Ordering::Acquire),
            native_rate: clock.native_sample_rate.load(Ordering::Acquire),
            running: clock.native_running.load(Ordering::Acquire) != 0,
            revision: taps.revision.load(Ordering::Acquire),
            graph_epoch: taps.graph_epoch.load(Ordering::Acquire),
            ready: taps.ready.load(Ordering::Acquire),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub context: Context,
    pub age: Option<ResidenceSnapshot>,
    pub faults: u64,
}

impl SourceRegistry {
    /// Called by the serialized graph owner after processing this source's cycle.
    pub fn record_content_latency(&self, token: u64, revision: u64, graph_epoch: u64, ready: bool) {
        let read = self.read(token);
        if let Some(source) = read.source() {
            let taps = &source.measurements;
            // Release publishes graph readiness to the control-side Acquire reads.
            taps.ready.store(ready, Ordering::Release);
            taps.graph_epoch.store(graph_epoch, Ordering::Release);
            taps.revision.store(revision, Ordering::Release);
            if !ready {
                taps.faults.fetch_add(1, Ordering::Release);
            }
        }
    }

    /// Bounded atomic observation; contention never becomes a new window baseline.
    #[must_use]
    pub fn content_latency(&self, token: u64) -> Option<Snapshot> {
        let read = self.read(token);
        let source = read.source()?;
        let before = Context::read(source);
        let age = source.measurements.age.snapshot();
        // Acquire pairs with the sole consumer's Release fault increments.
        let faults = source.measurements.faults.load(Ordering::Acquire);
        let context = Context::read(source);
        Some(Snapshot {
            context,
            age: (before == context).then_some(age).flatten(),
            faults,
        })
    }
}

pub(super) fn capture_age(
    source: &Source,
    epoch: u64,
    start: u64,
    frames: u32,
    label: u64,
) -> Option<u64> {
    let sum = (0..frames).try_fold(0_u128, |sum, offset| {
        // SAFETY: the consumer pins this mapping and checks the PCM epoch/lease
        // after copying. Every coordinate validates its epoch and absolute frame.
        let acquired = unsafe {
            source
                .mapping
                .clock()
                .time_at(source.capacity, epoch, start + u64::from(offset))
        }?;
        Some(sum + u128::from(label.checked_sub(acquired)?))
    })?;
    u64::try_from(sum.checked_div(u128::from(frames))?).ok()
}
