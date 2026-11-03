//! Waits for bounded shared PCM packets and runs native DSP without blocking the engine.

use super::mark_plugin_worker_failed;
use crate::{AudioLayout, WORKER_FAILED, latency, rack_formats, shared_transport::WorkerAudio};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// One processing block's duration at the prepared rate (48 kHz if it is unusable).
fn block_period(block_frames: usize, sample_rate: f64) -> Duration {
    let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        48_000.0
    };
    Duration::from_secs_f64(f64::from(u32::try_from(block_frames).unwrap_or(u32::MAX)) / rate)
}

/// Sleeps until the parent advances the word blocking the worker, at most two blocks (a
/// missed wake costs at most that). Polls where the shared wake API is unavailable, after
/// a failed wait, and for good after repeated failures.
#[derive(Default)]
struct Waiter {
    #[cfg(target_os = "macos")]
    failures: u32,
}

impl Waiter {
    const POLL: Duration = Duration::from_micros(100);
    #[cfg(target_os = "macos")]
    const MAXIMUM_FAILURES: u32 = 8;

    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            clippy::unused_self,
            reason = "macOS retains failed-futex state; other platforms use stateless polling"
        )
    )]
    fn wait(&mut self, word: &std::sync::atomic::AtomicU64, observed: u64, block: Duration) {
        #[cfg(target_os = "macos")]
        if self.failures < Self::MAXIMUM_FAILURES
            && let Some(futex) = mixrelay_realtime::shared_futex()
        {
            if let mixrelay_realtime::FutexWake::Failed(_) = futex.wait(word, observed, block * 2) {
                self.failures += 1;
            } else {
                self.failures = 0;
                return;
            }
        }
        let _ = (word, observed, block);
        thread::sleep(Self::POLL);
    }
}

pub(super) fn run(
    shared: &mut WorkerAudio,
    plugin: &Mutex<rack_formats::RackWorkerPlugin>,
    layout: AudioLayout,
    parent_closed: &AtomicBool,
    sample_rate: f64,
) {
    let block = block_period(layout.block_frames, sample_rate);
    // One block of plug-in processing per release: finish within 80% of it.
    #[cfg(target_os = "macos")]
    let _realtime = mixrelay_realtime::RealtimeThread::promote(block, block / 2, block * 4 / 5)
        .inspect_err(|error| tracing::warn!(%error, "plug-in DSP runs at standard priority"))
        .ok();
    let mut waiter = Waiter::default();
    loop {
        let header = shared.header();
        if parent_closed.load(Ordering::Acquire) || header.shutdown.load(Ordering::Acquire) != 0 {
            return;
        }
        // Choose what to wait for before the final readiness check: whichever parent
        // transition then happens either makes input pending or changes the chosen word.
        let (word, value) = shared.blocking_word(shared.wake_words());
        if !shared.input_pending() {
            waiter.wait(word, value, block);
            continue;
        }
        let mut plugin = match plugin.try_lock() {
            Ok(plugin) => plugin,
            Err(std::sync::TryLockError::WouldBlock) => {
                // The editor holds the instance briefly; sleep rather than spin at
                // realtime priority against a lower-priority holder.
                thread::sleep(Duration::from_micros(50));
                continue;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                mark_plugin_worker_failed(shared.header());
                return;
            }
        };
        // A main-thread restart may fail while DSP waits for exclusive instance access.
        // Recheck under that same lock before touching a now-deactivated native instance.
        if parent_closed.load(Ordering::Acquire)
            || shared.header().shutdown.load(Ordering::Acquire) != 0
            || shared.header().worker_status.load(Ordering::Acquire) == WORKER_FAILED
        {
            return;
        }
        process_packet(shared, &mut plugin, layout);
    }
}

pub(crate) fn process_packet(
    shared: &mut WorkerAudio,
    plugin: &mut rack_formats::RackWorkerPlugin,
    layout: AudioLayout,
) {
    let Some(frames) = shared.receive(|source| copy_shared_input_to_rack(source, plugin, layout))
    else {
        return;
    };
    let result = {
        let _operation = shared
            .header()
            .dsp_operation
            .enter(crate::native_operation::Stage::Process);
        plugin.process(frames)
    };
    if let Err(error) = result {
        if let Some(cause) = error.native_cause() {
            shared.header().process_faults.record_native(cause);
        }
        shared.header().process_faults.record(error.kind());
        plugin.bypass(frames);
    }
    latency::publish_intrinsic_frames(shared.header(), plugin.intrinsic_latency_frames());
    shared.publish(|destination| copy_rack_output_to_shared(destination, plugin, layout));
}

fn copy_shared_input_to_rack(
    source: &[f32],
    plugin: &mut rack_formats::RackWorkerPlugin,
    layout: AudioLayout,
) {
    let frames = source.len().checked_div(layout.inputs).unwrap_or(0);
    for (channel_index, channel) in plugin.inputs_mut().iter_mut().enumerate() {
        for frame in 0..frames {
            channel[frame] = source[frame * layout.inputs + channel_index];
        }
    }
}

fn copy_rack_output_to_shared(
    destination: &mut [f32],
    plugin: &rack_formats::RackWorkerPlugin,
    layout: AudioLayout,
) {
    let frames = destination.len().checked_div(layout.outputs).unwrap_or(0);
    for frame in 0..frames {
        for (channel_index, channel) in plugin.outputs().iter().enumerate() {
            destination[frame * layout.outputs + channel_index] = channel[frame];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_worker_waiter_returns_on_a_changed_word_and_polls_after_failures() {
        let word = std::sync::atomic::AtomicU64::new(3);
        let mut waiter = Waiter::default();
        let started = std::time::Instant::now();
        // A stale snapshot ends the wait at once.
        waiter.wait(&word, 2, Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
        // After repeated failures the futex is no longer trusted; a short poll remains.
        waiter.failures = Waiter::MAXIMUM_FAILURES;
        waiter.wait(&word, 3, Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn plug_in_blocks_last_their_frames_at_the_prepared_rate() {
        assert_eq!(
            block_period(128, 48_000.0),
            Duration::from_secs_f64(128.0 / 48_000.0)
        );
        // An unusable rate falls back to 48 kHz rather than a zero or infinite period.
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(block_period(480, rate), Duration::from_millis(10));
        }
    }
}
