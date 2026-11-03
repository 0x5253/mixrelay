use super::trace::{SinkError, Trace, TraceConfig};
use pipewire::stream::{Stream, StreamBox, StreamListener};
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};

/// Control-side phase state read by the input callback without locking.
#[derive(Clone)]
pub(super) struct PhaseState {
    pub(super) active: Arc<AtomicUsize>,
    pub(super) completed: Arc<AtomicUsize>,
    pub(super) invalid_buffers: Arc<AtomicU64>,
    pub(super) pcm_buffers: Arc<AtomicU64>,
}

impl PhaseState {
    pub(super) fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            completed: Arc::new(AtomicUsize::new(0)),
            invalid_buffers: Arc::new(AtomicU64::new(0)),
            pcm_buffers: Arc::new(AtomicU64::new(0)),
        }
    }
}

struct Recorder {
    trace: Option<Trace>,
    completed: SyncSender<Trace>,
    phase_state: PhaseState,
    current_phase: usize,
    current_frames: usize,
}

impl Recorder {
    fn process(&mut self, stream: &Stream) {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };
        let data_list = buffer.datas_mut();
        let [data] = data_list else {
            self.invalid();
            return;
        };
        let raw = data.as_raw();
        if raw.chunk.is_null() {
            self.invalid();
            return;
        }
        // SAFETY: `data` comes from this live dequeued buffer and owns a valid SPA
        // chunk pointer until the Buffer guard is dropped and requeues it below.
        let chunk = unsafe { &*raw.chunk };
        let Ok(offset) = usize::try_from(chunk.offset) else {
            self.invalid();
            return;
        };
        let Ok(size) = usize::try_from(chunk.size) else {
            self.invalid();
            return;
        };
        let Some(is_empty) = self.classify_chunk(chunk.flags) else {
            return;
        };
        let Ok(maxsize) = usize::try_from(raw.maxsize) else {
            self.invalid();
            return;
        };
        let Some(range) = valid_chunk_range(offset, size, maxsize) else {
            self.invalid();
            return;
        };
        if !valid_stereo_chunk(chunk.stride, size) {
            self.invalid();
            return;
        }
        if is_empty {
            self.store(None, size / 8, true);
            return;
        }
        let Some(bytes) = data.data() else {
            self.invalid();
            return;
        };
        let Some(bytes) = bytes.get(range) else {
            self.invalid();
            return;
        };
        self.store(Some(bytes), size / 8, false);
        // `Buffer`'s Drop requeues the single dequeued PipeWire buffer.
    }

    fn store(&mut self, bytes: Option<&[u8]>, available_frames: usize, is_empty: bool) {
        if !is_empty && bytes.is_none() {
            self.invalid();
            return;
        }
        let phase = self.phase_state.active.load(Ordering::Acquire);
        if !is_empty && available_frames != 0 {
            // process() has validated the mapped buffer before calling store().
            // Sample the phase first so this readiness receipt cannot retroactively
            // include the setup buffer. Recorder/bytes remain callback-owned;
            // control sees only atomics, never raw pointers or mutable trace state.
            self.phase_state.pcm_buffers.fetch_add(1, Ordering::Release);
        }
        let Some(trace) = self.trace.as_ref() else {
            return;
        };
        if phase == 0 || phase > trace.frame_counts.len() {
            return;
        }
        if self.current_phase != phase {
            if phase != self.current_phase + 1 {
                self.invalid();
                return;
            }
            self.current_phase = phase;
            self.current_frames = 0;
        }
        let remaining = trace
            .config
            .phase_frames
            .saturating_sub(self.current_frames);
        let (frames, post_window_frames) = phase_window_frames(available_frames, remaining);
        let start = (phase - 1) * trace.config.phase_frames * 2 + self.current_frames * 2;
        if let Some(trace) = self.trace.as_mut() {
            if is_empty {
                trace.samples[start..start + frames * 2].fill(0.0);
                trace.empty_frames[phase - 1] = trace.empty_frames[phase - 1]
                    .saturating_add(u32::try_from(frames).unwrap_or(u32::MAX));
            } else if let Some(bytes) = bytes {
                for (frame_index, frame) in bytes.chunks_exact(8).take(frames).enumerate() {
                    let offset = start + frame_index * 2;
                    trace.samples[offset] =
                        f32::from_ne_bytes([frame[0], frame[1], frame[2], frame[3]]);
                    trace.samples[offset + 1] =
                        f32::from_ne_bytes([frame[4], frame[5], frame[6], frame[7]]);
                }
            }
            trace.frame_counts[phase - 1] =
                u32::try_from(self.current_frames + frames).unwrap_or(u32::MAX);
            trace.post_window_frames[phase - 1] = trace.post_window_frames[phase - 1]
                .saturating_add(u64::try_from(post_window_frames).unwrap_or(u64::MAX));
        }
        self.current_frames += frames;
        if self
            .trace
            .as_ref()
            .is_some_and(|trace| self.current_frames == trace.config.phase_frames)
        {
            self.phase_state.active.store(0, Ordering::Release);
            self.phase_state.completed.store(phase, Ordering::Release);
        }
    }

    fn classify_chunk(&mut self, flags: i32) -> Option<bool> {
        let silence = chunk_silence(flags);
        if silence.is_none() {
            self.invalid();
        }
        silence
    }

    fn invalid(&mut self) {
        self.phase_state
            .invalid_buffers
            .fetch_add(1, Ordering::Relaxed);
        if let Some(trace) = self.trace.as_mut() {
            trace.invalid_buffers = trace.invalid_buffers.saturating_add(1);
        }
    }
}

fn phase_window_frames(available: usize, remaining: usize) -> (usize, usize) {
    let measured = available.min(remaining);
    (measured, available - measured)
}

fn valid_stereo_chunk(stride: i32, bytes: usize) -> bool {
    stride == 8 && bytes.is_multiple_of(8)
}

fn chunk_silence(flags: i32) -> Option<bool> {
    if flags & pipewire::spa::sys::SPA_CHUNK_FLAG_CORRUPTED.cast_signed() != 0 {
        return None;
    }
    Some(flags & pipewire::spa::sys::SPA_CHUNK_FLAG_EMPTY.cast_signed() != 0)
}

fn valid_chunk_range(offset: usize, size: usize, maxsize: usize) -> Option<std::ops::Range<usize>> {
    let end = offset.checked_add(size)?;
    (end <= maxsize).then_some(offset..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace;

    #[test]
    fn variable_packets_measure_exact_window_and_account_only_final_packet_tail() {
        for packet_sizes in [&[256][..], &[1024], &[127, 511, 73, 2048]] {
            let mut remaining = trace::PHASE_FRAMES;
            let mut measured = 0;
            let mut post_window = 0;
            let mut packet_index = 0;
            while remaining != 0 {
                let available = packet_sizes[packet_index % packet_sizes.len()];
                let (accepted, tail) = phase_window_frames(available, remaining);
                measured += accepted;
                remaining -= accepted;
                post_window += tail;
                packet_index += 1;
            }
            assert_eq!(measured, trace::PHASE_FRAMES);
            assert!(post_window < *packet_sizes.iter().max().unwrap());
        }
    }

    #[test]
    fn malformed_stereo_chunks_are_rejected_instead_of_counted_as_pcm() {
        assert!(valid_stereo_chunk(8, 0));
        assert!(valid_stereo_chunk(8, 16));
        assert!(!valid_stereo_chunk(4, 16));
        assert!(!valid_stereo_chunk(-8, 16));
        assert!(!valid_stereo_chunk(8, 15));
    }

    #[test]
    fn corrupted_chunks_are_rejected_and_empty_chunks_are_classified_as_silence() {
        let empty = pipewire::spa::sys::SPA_CHUNK_FLAG_EMPTY.cast_signed();
        let corrupted = pipewire::spa::sys::SPA_CHUNK_FLAG_CORRUPTED.cast_signed();
        assert_eq!(chunk_silence(0), Some(false));
        assert_eq!(chunk_silence(empty), Some(true));
        assert_eq!(chunk_silence(corrupted), None);
        assert_eq!(chunk_silence(empty | corrupted), None);

        let phase_state = PhaseState::new();
        let (completed, _receiver) = sync_channel(1);
        let mut recorder = Recorder {
            trace: Some(Trace::new(TraceConfig::source_mute()).unwrap()),
            completed,
            phase_state: phase_state.clone(),
            current_phase: 0,
            current_frames: 0,
        };
        assert_eq!(recorder.classify_chunk(corrupted), None);
        assert_eq!(phase_state.invalid_buffers.load(Ordering::Acquire), 1);
        assert_eq!(recorder.trace.as_ref().unwrap().invalid_buffers, 1);
    }

    #[test]
    fn empty_chunk_records_neutral_zeros_and_counts_actual_empty_frames() {
        let phase_state = PhaseState::new();
        phase_state.active.store(1, Ordering::Release);
        let (completed, _receiver) = sync_channel(1);
        let mut recorder = Recorder {
            trace: Some(Trace::new(TraceConfig::source_mute()).unwrap()),
            completed,
            phase_state,
            current_phase: 0,
            current_frames: 0,
        };
        let nonzero_backing_memory = [0xff; 16];
        recorder.store(Some(&nonzero_backing_memory), 2, true);
        let trace = recorder.trace.as_ref().unwrap();
        assert_eq!(&trace.samples[..4], &[0.0; 4]);
        assert_eq!(trace.frame_counts[0], 2);
        assert_eq!(trace.empty_frames[0], 2);
    }

    #[test]
    fn running_pcm_readiness_does_not_record_setup_or_empty_buffers() {
        let phase_state = PhaseState::new();
        let (completed, _receiver) = sync_channel(1);
        let mut recorder = Recorder {
            trace: Some(Trace::new(TraceConfig::graph_continuity()).unwrap()),
            completed,
            phase_state: phase_state.clone(),
            current_phase: 0,
            current_frames: 0,
        };
        recorder.store(None, 2, true);
        recorder.store(Some(&[]), 0, false);
        assert_eq!(phase_state.pcm_buffers.load(Ordering::Acquire), 0);
        recorder.store(Some(&[0; 16]), 2, false);
        assert_eq!(phase_state.pcm_buffers.load(Ordering::Acquire), 1);
        assert_eq!(recorder.trace.as_ref().unwrap().frame_counts[0], 0);
        phase_state.active.store(1, Ordering::Release);
        recorder.store(Some(&[0; 16]), 2, false);
        assert_eq!(recorder.trace.as_ref().unwrap().frame_counts[0], 2);
    }

    #[test]
    fn native_chunk_range_must_fit_the_declared_data_capacity() {
        assert_eq!(valid_chunk_range(8, 16, 32), Some(8..24));
        assert_eq!(valid_chunk_range(16, 16, 32), Some(16..32));
        assert!(valid_chunk_range(16, 17, 32).is_none());
        assert!(valid_chunk_range(usize::MAX, 1, usize::MAX).is_none());
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(trace) = self.trace.take() {
            let _ = self.completed.try_send(trace);
        }
    }
}

/// Stream/listener owner; output evidence returns only after native disconnection.
pub(super) struct CallbackSink<'core> {
    stream: StreamBox<'core>,
    listener: Option<StreamListener<Recorder>>,
    completed: Receiver<Trace>,
    phase_state: PhaseState,
    config: TraceConfig,
    finished: bool,
}

impl<'core> CallbackSink<'core> {
    pub(super) fn with_config(
        stream: StreamBox<'core>,
        config: TraceConfig,
    ) -> Result<Self, SinkError> {
        let phase_state = PhaseState::new();
        let (sender, completed) = sync_channel(1);
        let callback_state = Recorder {
            trace: Some(Trace::new(config)?),
            completed: sender,
            phase_state: phase_state.clone(),
            current_phase: 0,
            current_frames: 0,
        };
        let listener = stream
            .add_local_listener_with_user_data(callback_state)
            .process(|stream, state| state.process(stream))
            .register()?;
        Ok(Self {
            stream,
            listener: Some(listener),
            completed,
            phase_state,
            config,
            finished: false,
        })
    }

    pub(super) fn stream(&self) -> &Stream {
        &self.stream
    }

    pub(super) fn phase_state(&self) -> &PhaseState {
        &self.phase_state
    }

    pub(super) const fn config(&self) -> TraceConfig {
        self.config
    }

    pub(super) fn finish(mut self) -> Result<Trace, SinkError> {
        self.stream.disconnect()?;
        drop(self.listener.take());
        self.finished = true;
        self.completed
            .try_recv()
            .map_err(|_| SinkError::TraceReturn)
    }
}

impl Drop for CallbackSink<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stream.disconnect();
            drop(self.listener.take());
        }
    }
}
