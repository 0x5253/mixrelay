//! Low-rate, control-thread snapshots. No logging or property IPC on callbacks.
use super::{MacDevices, ffi, native::Native};
use mixrelay_macos_transport::device_io::CLOCK_BREAK_CAPACITY;
use std::sync::atomic::Ordering;

impl MacDevices {
    /// Reads atomic FIFO/progress counters while retaining each native owner.
    /// Called only by the format monitor; does not lock or pause a PCM producer.
    pub fn trace_audio(&self) {
        let owners: Vec<_> = {
            let Ok(cache) = self.cache.try_lock() else {
                return;
            };
            cache
                .values()
                .filter_map(|(_, owner)| owner.upgrade())
                .collect()
        };
        for device in owners {
            device.native.pcm_metrics.report(device.native.log_id);
            device.native.trace_clock_breaks();
            let pointer = device.native.pointer.as_ptr();
            // SAFETY: the local Arc retains initialized Native storage. These
            // accessors read atomics only, never the worker-owned control cell.
            let (native_frames, graph_frames, native_queued, graph_queued, overflows, clock_gaps) = unsafe {
                (
                    ffi::mr_device_io_native_frames(pointer),
                    ffi::mr_device_io_graph_frames(pointer),
                    ffi::mr_device_io_queued(pointer, false),
                    ffi::mr_device_io_queued(pointer, true),
                    ffi::mr_device_io_overflows(pointer),
                    ffi::mr_device_io_clock_discontinuities(pointer),
                )
            };
            // SAFETY: same retained owner and atomic-only observation. Select
            // the deadline-facing consumer, not the worker's empty polls.
            let pcm_starvations = unsafe { ffi::mr_device_io_starvations(pointer, device.input) };
            tracing::info!(target: "audio_trace", owner = device.native.log_id,
                input = device.input, enabled = device.activity.enabled(),
                ready = device.ready(), epoch = device.epoch.load(Ordering::Acquire),
                clock_id = device.clock_identity.load(Ordering::Acquire),
                native_rate = device.native_rate.load(Ordering::Acquire),
                graph_rate = device.graph_rate.load(Ordering::Acquire),
                native_frames, graph_frames, native_queued, graph_queued, overflows, clock_gaps,
                pcm_starvations,
                "native PCM queues");
        }
    }
}

impl Native {
    // Only the format monitor advances this cursor. Other diagnostic readers can
    // retain their own cursor without consuming this monitor's history.
    fn trace_clock_breaks(&self) {
        let mut cursor = self.clock_trace_cursor.load(Ordering::Relaxed);
        for _ in 0..CLOCK_BREAK_CAPACITY {
            let mut record = ffi::ClockBreakRecord::default();
            // SAFETY: trace_audio's device Arc retains Native and its atomic journal;
            // the complete local output is exclusive and disjoint from that owner.
            if !unsafe {
                ffi::mr_device_io_next_clock_break(self.pointer.as_ptr(), cursor, &raw mut record)
            } {
                break;
            }
            let event = record.event;
            tracing::info!(target: "audio_trace", owner = self.log_id,
                count = event.count, expected_sample = event.expected_sample,
                actual_sample = event.actual_sample,
                delta_frames = event.actual_sample - event.expected_sample,
                previous_host = event.previous_host, actual_host = event.actual_host,
                flags = event.flags, callback_host = event.callback_host,
                callback_flags = event.callback_flags, missed_before = record.missed_before,
                previous_enter = event.previous_enter, previous_exit = event.previous_exit,
                callback_enter = event.callback_enter,
                "native clock break evidence");
            cursor = record.next_cursor;
            self.clock_trace_cursor.store(cursor, Ordering::Relaxed);
        }
    }
}
