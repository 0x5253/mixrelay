use pipewire::stream::Time;
use serde::Serialize;
use std::{fs::OpenOptions, io::BufWriter, io::Write, path::Path, time::Instant};

/// Values copied from one native time report, without extrapolation across clocks.
#[derive(Clone, Copy, Default, Serialize)]
pub(super) struct NativeTime {
    report_monotonic_ns: i64,
    ticks: u64,
    rate_num: u32,
    rate_denom: u32,
    delay_ticks: i64,
    // PipeWire defines this as the sum of queued pw_buffer.size, not chunk bytes.
    queued_size_sum: u64,
    resampler_samples: u64,
    queued_buffers: u32,
    available_buffers: u32,
}

impl From<Time> for NativeTime {
    fn from(value: Time) -> Self {
        Self {
            report_monotonic_ns: value.now(),
            ticks: value.ticks(),
            rate_num: value.rate().num,
            rate_denom: value.rate().denom,
            delay_ticks: value.delay(),
            queued_size_sum: value.queued(),
            resampler_samples: value.buffered(),
            queued_buffers: value.queued_buffers(),
            available_buffers: value.avail_buffers(),
        }
    }
}

/// One callback record. Zero produced frames retains a missing/unusable buffer event.
#[derive(Clone, Copy, Default, Serialize)]
pub(super) struct Event {
    pub(super) sequence: u64,
    pub(super) elapsed_ns: u128,
    pub(super) work_ns: u128,
    pub(super) first_frame: u64,
    pub(super) requested_frames: Option<u64>,
    pub(super) capacity_bytes: Option<u32>,
    pub(super) produced_frames: u32,
    pub(super) native_time: Option<NativeTime>,
}

/// Fixed, prefaulted callback storage. Serialization happens only after disconnection.
pub(super) struct Trace {
    started: Instant,
    events: Box<[Event]>,
    used: usize,
    dropped: u64,
}

impl Trace {
    /// Allocates/touches every record before the stream can produce a callback.
    pub(super) fn new(capacity: u32) -> Self {
        let events = (0..capacity)
            .map(|index| Event {
                sequence: u64::from(index) + 1,
                ..Event::default()
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            started: Instant::now(),
            events,
            used: 0,
            dropped: 0,
        }
    }

    /// Local elapsed time; native monotonic report time is recorded separately.
    pub(super) fn elapsed_ns(&self) -> u128 {
        self.started.elapsed().as_nanos()
    }

    /// Never grows storage or overwrites previous evidence when capacity is exhausted.
    pub(super) fn record(&mut self, mut event: Event) {
        if let Some(slot) = self.events.get_mut(self.used) {
            event.sequence = slot.sequence;
            *slot = event;
            self.used += 1;
        } else {
            self.dropped += 1;
        }
    }

    /// Writes a new trace file off the callback after its final invocation is drained.
    /// # Errors
    /// Rejects an existing path and propagates serialization/write/flush failures.
    pub(super) fn save(
        &self,
        path: &Path,
        rate: u16,
        dispatch: &'static str,
    ) -> Result<(), super::source::SourceError> {
        #[derive(Serialize)]
        struct Evidence<'a> {
            sample_rate: u16,
            channels: u16,
            right_channel: &'static str,
            callback_dispatch: &'static str,
            process_id: u32,
            dropped_records: u64,
            events: &'a [Event],
        }
        let mut file = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
        serde_json::to_writer(
            &mut file,
            &Evidence {
                sample_rate: rate,
                channels: 2,
                right_channel: "frame-sequence-f32-23bit",
                callback_dispatch: dispatch,
                process_id: std::process::id(),
                dropped_records: self.dropped,
                events: &self.events[..self.used],
            },
        )?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_fixed_and_excess_events_are_counted_without_overwriting() {
        let mut trace = Trace::new(2);
        let allocation = trace.events.as_ptr();
        for first_frame in [10, 20, 30] {
            trace.record(Event {
                first_frame,
                ..Event::default()
            });
        }
        assert_eq!(trace.events.as_ptr(), allocation);
        assert_eq!(trace.events.len(), 2);
        assert_eq!(trace.used, 2);
        assert_eq!(trace.dropped, 1);
        assert_eq!(trace.events[0].first_frame, 10);
        assert_eq!(trace.events[1].first_frame, 20);
        assert_eq!(trace.events[1].sequence, 2);
    }
}
