//! Callback-owned clock conversion and bounded queue policy; prepared off realtime.
#[cfg(test)]
mod tests;
use super::{
    ffi::{
        mr_device_io_bridge_discard, mr_device_io_bridge_read, mr_device_io_clock_discontinuities,
        mr_device_io_graph_frames, mr_device_io_native_frames, mr_device_io_output_epoch,
        mr_device_io_overflows, mr_device_io_quantum, mr_device_io_queued,
    },
    worker::WorkerContext,
};
use mixrelay_audio_io::buffer_policy::{MAXIMUM_TARGET_FRAMES, QueueBounds, QueueTargetPolicy};
use mixrelay_audio_io::clock_bridge::{
    AsyncClockBridge, BridgeConfig, BridgeError, ClockFeedback, ClockRelation, Discontinuity,
    FeedbackQueue, TransferState,
};
use std::sync::atomic::{AtomicU64, Ordering};
pub(super) const PUMP_FRAMES: usize = 256;
pub(super) const QUEUE_CAPACITY: usize = mixrelay_audio_io::retention::RING_FRAMES as usize;
pub(super) static NEXT_DEVICE_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Prepared on control, then exclusively borrowed by the native PCM callback.
/// Graph revisions share queue handles, never mutable conversion or scratch.
pub(super) struct DeviceBridge {
    source_times: Vec<u64>,
    destination_times: Vec<u64>,
    pub(super) bridge: AsyncClockBridge,
    pub(super) source: Vec<f32>,
    pub(super) destination: Vec<f32>,
    pub(super) graph_rate: u32,
    pub(super) native_rate: u32,
    pub(super) clock_aligned: bool,
    graph_queue_target: QueueTargetPolicy,
    native_queue_target: QueueTargetPolicy,
    pub(super) epoch: u64,
    feedback_frames: u64,
    residence_frames: u64,
    pub(super) streaming: bool,
    pub(super) transport_overflows: u64,
    pub(super) clock_discontinuities: u64,
}
impl DeviceBridge {
    /// Transport loss invalidates PCM/history, not the prepared sinc tables.
    /// The PCM owner reuses all allocations while publishing a fresh queue epoch.
    pub(super) fn restart(&mut self, context: &WorkerContext) {
        self.epoch = NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed);
        self.bridge.reset(self.epoch, Discontinuity::TransportLoss);
        self.streaming = false;
        self.activate(context);
    }
    pub(super) fn synchronize_activity(&mut self, context: &WorkerContext) -> bool {
        context.activity.synchronize(|| {
            context
                .native
                .pcm_metrics
                .reset(super::worker::metrics::ResetCause::Activity);
            self.epoch = NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed);
            self.bridge.reset(self.epoch, Discontinuity::StreamRestart);
            self.streaming = false;
            self.activate(context);
            // Only graph FIFO freshness changes; the native clock still runs.
            self.epoch
        })
    }
    pub(super) fn new(context: &WorkerContext, native_rate: u32) -> Result<Self, String> {
        let graph_rate = context.graph_rate.load(Ordering::Acquire);
        let mut result = Self::prepare(
            context.channels,
            context.input,
            native_rate,
            graph_rate,
            context.clock_aligned.load(Ordering::Acquire),
        )?;
        result.activate(context);
        Ok(result)
    }
    pub(super) fn prepare(
        channels: usize,
        input: bool,
        native_rate: u32,
        graph_rate: u32,
        clock_aligned: bool,
    ) -> Result<Self, String> {
        let (input_rate, output_rate) = if input {
            (native_rate, graph_rate)
        } else {
            (graph_rate, native_rate)
        };
        // Alignment is supplied by the active device set: the pacing owner or
        // a verified second IOProc of that same device. Nominal Hz is not proof.
        let clock_relation = if clock_aligned {
            ClockRelation::ProvenSameClock
        } else {
            ClockRelation::Independent
        };
        let mut bridge = AsyncClockBridge::new(BridgeConfig {
            channels,
            input_rate,
            output_rate,
            max_input_frames: PUMP_FRAMES,
            max_output_frames: PUMP_FRAMES,
            capacity_frames: QUEUE_CAPACITY,
            priming_frames: 32,
            clock_relation,
            feedback_queue: FeedbackQueue::Input,
            max_correction_ppm: 1_000.0,
        })
        .map_err(|error| error.to_string())?;
        let epoch = NEXT_DEVICE_EPOCH.fetch_add(1, Ordering::Relaxed);
        bridge.reset(epoch, Discontinuity::StreamRestart);
        let bounds = QueueBounds {
            minimum_frames: 32,
            capacity_frames: QUEUE_CAPACITY,
            maximum_target_frames: MAXIMUM_TARGET_FRAMES,
        };
        let graph_queue_target =
            QueueTargetPolicy::new(graph_rate, native_rate, clock_relation, bounds)
                .map_err(|error| error.to_string())?;
        let native_queue_target =
            QueueTargetPolicy::new(native_rate, graph_rate, clock_relation, bounds)
                .map_err(|error| error.to_string())?;
        Ok(Self {
            source_times: mixrelay_audio_io::pcm_buffer::timestamps(PUMP_FRAMES)
                .map_err(|error| error.to_string())?,
            destination_times: mixrelay_audio_io::pcm_buffer::timestamps(PUMP_FRAMES)
                .map_err(|error| error.to_string())?,
            bridge,
            source: mixrelay_audio_io::pcm_buffer::zeroed(PUMP_FRAMES * channels)
                .map_err(|error| error.to_string())?,
            destination: mixrelay_audio_io::pcm_buffer::zeroed(PUMP_FRAMES * channels)
                .map_err(|error| error.to_string())?,
            graph_rate,
            native_rate,
            clock_aligned,
            graph_queue_target,
            native_queue_target,
            epoch,
            feedback_frames: 0,
            residence_frames: 0,
            streaming: false,
            transport_overflows: 0,
            clock_discontinuities: 0,
        })
    }
    pub(super) fn activate(&mut self, context: &WorkerContext) {
        context
            .native
            .latency
            .faults
            .fetch_add(1, Ordering::Release);
        context.bridge_ready.store(false, Ordering::Release);
        let pointer = context.native.pointer.as_ptr();
        // SAFETY: Native is retained and these progress/queue snapshots are atomic.
        let native_frames = unsafe { mr_device_io_native_frames(pointer) };
        self.residence_frames = native_frames;
        // SAFETY: the PCM owner exclusively accesses its queue consumer; all counter reads are atomic
        // and the retained native resource outlives this activation.
        unsafe {
            mr_device_io_bridge_discard(pointer);
            mr_device_io_output_epoch(pointer, self.epoch);
            self.feedback_frames = if context.input {
                native_frames
            } else {
                mr_device_io_graph_frames(pointer)
            };
            self.transport_overflows = mr_device_io_overflows(pointer);
            self.clock_discontinuities = mr_device_io_clock_discontinuities(pointer);
        }
    }

    pub(super) fn observe(
        &mut self,
        context: &WorkerContext,
        native_running: bool,
    ) -> Result<(), BridgeError> {
        let pointer = context.native.pointer.as_ptr();
        // Observe all pending input, not just a destination FIFO clamped by pump().
        // Otherwise backpressure hides source drift until the native ring overflows.
        // SAFETY: the PCM owner retains Native; these reads are bounded atomics.
        let frames = unsafe {
            if context.input {
                mr_device_io_native_frames(pointer)
            } else {
                mr_device_io_graph_frames(pointer)
            }
        };
        let elapsed = frames.wrapping_sub(self.feedback_frames);
        // Residence advances with the actual native callback clock, even when
        // the graph producer has not supplied any new PCM in this invocation.
        // Areas use nominal callback time in both converter domains; the
        // reporter applies the direction's measured scalar to both residences.
        let native_frames = unsafe { mr_device_io_native_frames(pointer) };
        let native_elapsed = native_frames.wrapping_sub(self.residence_frames);
        self.residence_frames = native_frames;
        self.bridge
            .advance_residence(std::time::Duration::from_nanos(
                native_elapsed.saturating_mul(1_000_000_000) / u64::from(self.native_rate),
            ));
        self.feedback_frames = frames;
        if elapsed == 0 {
            return Ok(());
        }
        let rate = if context.input {
            self.native_rate
        } else {
            self.graph_rate
        };
        let (queued, target) = self.feedback_occupancy(context);
        self.bridge.observe_clock(ClockFeedback {
            queue: FeedbackQueue::Input,
            epoch: self.epoch,
            queued_frames: queued,
            target_frames: target,
            elapsed_frames: usize::try_from(elapsed)
                .unwrap_or(usize::MAX)
                .min(rate as usize),
            clock_running: native_running && self.streaming && !self.clock_aligned,
        })?;
        Ok(())
    }

    pub(super) fn feedback_occupancy(&self, context: &WorkerContext) -> (usize, usize) {
        let pointer = context.native.pointer.as_ptr();
        // SAFETY: retained native resource, atomic queue lengths only. There is
        // one PCM owner, so moving frames between these FIFOs preserves the sum.
        let source = unsafe { mr_device_io_queued(pointer, !context.input) } as usize;
        let staged = self.bridge.buffered_input_frames();
        if !context.input {
            return (source + staged, self.queue_target(context, true) / 2);
        }
        let output = unsafe { mr_device_io_queued(pointer, true) } as usize;
        let output_in_source =
            (output * self.native_rate as usize).div_ceil(self.graph_rate as usize);
        // One observed graph burst plus one converter input slab. The physical
        // ring capacity is overflow protection, never the target latency.
        // Alignment intentionally retains history. Do not mistake that reserve
        // for producer drift and accelerate a correctly paced source toward it.
        let window =
            unsafe { mixrelay_macos_transport::device_io::capture_window(pointer.cast()) } as usize;
        let alignment_target =
            (window * self.native_rate as usize).div_ceil(self.graph_rate as usize);
        let target = (self.queue_target(context, false) / 2 + self.bridge.processing_quantum().0)
            .max(alignment_target);
        (source + staged + output_in_source, target)
    }

    /// Bound buffering by the actual producer/consumer burst, converted to the
    /// destination rate. A 32-sample graph still needs to absorb one complete
    /// hardware callback; it must not silently inherit a fixed 2048-sample FIFO.
    pub(super) fn queue_target(&self, context: &WorkerContext, graph: bool) -> usize {
        let pointer = context.native.pointer.as_ptr();
        // SAFETY: this PCM owner retains Native; only atomic progress is read.
        let native_quantum = unsafe { mr_device_io_quantum(pointer, false) };
        // SAFETY: the same retained native queue publishes its graph quantum atomically.
        let graph_quantum = unsafe { mr_device_io_quantum(pointer, true) }
            .max(context.graph_quantum.load(Ordering::Acquire));
        // A sinc converter produces a complete chunk, not one DSP/native
        // callback. A smaller destination target strands the rest behind this
        // next conversion invocation while the realtime consumer has already run dry.
        let (source_chunk, destination_chunk) = self.bridge.processing_quantum();
        let converter_burst = if graph == context.input {
            destination_chunk
        } else {
            source_chunk
        };
        if graph {
            let target = self.graph_queue_target.target_frames(
                graph_quantum,
                native_quantum,
                converter_burst,
            );
            let reserve = if context.input {
                (unsafe { mixrelay_macos_transport::device_io::capture_window(pointer.cast()) })
                    as usize
            } else {
                0
            };
            target.max(reserve)
        } else {
            self.native_queue_target
                .target_frames(native_quantum, graph_quantum, converter_burst)
        }
    }

    fn stage_input(&mut self, context: &WorkerContext, count: usize) -> Result<(), BridgeError> {
        let pointer = context.native.pointer.as_ptr();
        let count = if context.input {
            unsafe {
                mixrelay_macos_transport::device_io::bridge_read_timed(
                    pointer.cast(),
                    &mut self.source[..count * context.channels],
                    &mut self.source_times[..count],
                )
            }
        } else {
            unsafe {
                mr_device_io_bridge_read(
                    pointer,
                    self.source.as_mut_ptr(),
                    u32::try_from(count).unwrap_or(0),
                )
            }
        } as usize;
        if count != 0 {
            context
                .native
                .pcm_metrics
                .transferred(u32::try_from(count).unwrap_or(0), 0);
            if context.input {
                let faults = mixrelay_audio_io::pcm::sanitize_samples(
                    &mut self.source[..count * context.channels],
                );
                context.native.source_sample_faults.record(faults);
            }
            if context.input {
                self.bridge.push_timed_interleaved(
                    &self.source[..count * context.channels],
                    &self.source_times[..count],
                )?;
            } else {
                self.bridge
                    .push_interleaved(&self.source[..count * context.channels])?;
            }
        }
        Ok(())
    }

    pub(super) fn pump(&mut self, context: &WorkerContext) -> Result<(), BridgeError> {
        let pointer = context.native.pointer.as_ptr();
        let destination_graph = context.input;
        let target = self.queue_target(context, destination_graph);
        // Bound each scheduling turn even at extreme rate ratios. Backpressure
        // stays in the real clock-crossing FIFO, not a greedily drained private
        // staging queue. Priming zeros are never enqueued as valid PCM.
        for _ in 0..64 {
            let queued = unsafe { mr_device_io_queued(pointer, destination_graph) } as usize;
            if queued >= target {
                break;
            }
            let source_frames =
                unsafe { mr_device_io_queued(pointer, !destination_graph) } as usize;
            if self.bridge.queued_input_frames() < PUMP_FRAMES * 2 && source_frames != 0 {
                self.stage_input(context, source_frames.min(PUMP_FRAMES))?;
            }
            let wanted = (target - queued).min(PUMP_FRAMES);
            let report = self.bridge.pull_timed_interleaved(
                &mut self.destination[..wanted * context.channels],
                &mut self.destination_times[..wanted],
            )?;
            if let Some(value) = report.input_residence {
                context.native.latency.src_input.publish(value);
            }
            if let Some(value) = report.output_residence {
                context.native.latency.src_output.publish(value);
            }
            context
                .native
                .latency
                .src_delay
                .store(report.algorithmic_delay_frames as u64, Ordering::Release);
            if report.state == TransferState::Priming {
                context
                    .native
                    .latency
                    .faults
                    .fetch_add(1, Ordering::Release);
            }
            self.streaming = report.state != TransferState::Priming;
            if report.transferred_frames == 0 {
                break;
            }
            let count =
                u32::try_from(report.transferred_frames).map_err(|_| BridgeError::InvalidBuffer)?;
            if unsafe {
                mixrelay_macos_transport::device_io::bridge_write_timed(
                    pointer.cast(),
                    &self.destination[..count as usize * context.channels],
                    &self.destination_times[..count as usize],
                )
            } != count
            {
                return Err(BridgeError::Overflow);
            }
            context.bridge_ready.store(true, Ordering::Release);
            context.native.pcm_metrics.transferred(0, count);
        }
        Ok(())
    }
}
