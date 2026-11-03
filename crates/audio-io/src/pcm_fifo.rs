//! Bounded interleaved PCM with one producer role and one consumer role.
//! Storage and geometry are prepared before audio starts; transfers neither wait
//! nor allocate. Queue occupancy and consumed clock frames are separate counters.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

/// Invalid or unallocatable off-realtime queue geometry.
#[derive(Debug, thiserror::Error)]
pub enum PcmFifoError {
    /// Audio channel count must fit the engine's supported channel bound.
    #[error("PCM queue requires 1..=64 channels")]
    Channels,
    /// Capacity is measured in complete frames, independent of channel width.
    #[error("PCM queue requires 1..=65536 frames")]
    Capacity,
    /// Allocation failure is returned before either role can be published.
    #[error("PCM queue allocation failed: {0}")]
    Allocation(#[from] std::collections::TryReserveError),
}

/// Independent atomic observations, not a transactional clock snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFifoStats {
    /// Complete interleaved frames currently queued; incoherent observations are zero.
    pub queued_frames: usize,
    /// Cumulative accepted frames; wraps at `u64::MAX` independently of ring positions.
    pub written_frames: u64,
    /// Cumulative frames actually read. Discard never advances this clock observation.
    pub consumed_frames: u64,
}

struct Shared {
    samples: Box<[AtomicU32]>,
    channels: usize,
    capacity: usize,
    // Positions cycle through twice the frame capacity. Unlike cumulative clock
    // counters, these cannot overflow or misindex a non-power-of-two capacity.
    published: AtomicUsize,
    released: AtomicUsize,
    written: AtomicU64,
    consumed: AtomicU64,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Owner diagnostics must not dump an entire buffer of captured audio.
        formatter
            .debug_struct("PcmFifo")
            .field("channels", &self.channels)
            .field("capacity_frames", &self.capacity)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// Sole write role. It is not cloneable; sharing its owner does not grant a second
/// concurrent writer: overlapping operations fail immediately with zero transfer.
#[derive(Debug)]
pub struct PcmProducer {
    shared: Arc<Shared>,
    busy: AtomicBool,
}

/// Sole read/discard role. It is not cloneable. A bounded operation guard also
/// excludes overlapping calls through shared native-owner references.
#[derive(Debug)]
pub struct PcmConsumer {
    shared: Arc<Shared>,
    busy: AtomicBool,
}

/// Prepare one fixed-width queue and transfer its two roles to their thread owners.
/// Neither role exposes the storage or grants another producer/consumer. Dropping
/// the final role frees storage, so native owners must drain before off-RT teardown.
///
/// # Errors
/// Returns [`PcmFifoError`] for invalid geometry or failed storage allocation.
///
/// # Examples
/// ```
/// use mixrelay_audio_io::pcm_fifo::pcm_fifo;
/// let (producer, consumer) = pcm_fifo(2, 4096)?;
/// assert_eq!(producer.write(&[0.25, -0.25]), 1);
/// let mut output = [1.0; 4];
/// assert_eq!(consumer.read(&mut output), 1);
/// assert_eq!(output, [0.25, -0.25, 0.0, 0.0]);
/// # Ok::<(), mixrelay_audio_io::pcm_fifo::PcmFifoError>(())
/// ```
pub fn pcm_fifo(
    channels: usize,
    capacity_frames: usize,
) -> Result<(PcmProducer, PcmConsumer), PcmFifoError> {
    if !(1..=64).contains(&channels) {
        return Err(PcmFifoError::Channels);
    }
    if !(1..=65536).contains(&capacity_frames) {
        return Err(PcmFifoError::Capacity);
    }
    let mut samples = Vec::new();
    samples.try_reserve_exact(channels * capacity_frames)?;
    samples.resize_with(channels * capacity_frames, || AtomicU32::new(0));
    let shared = Arc::new(Shared {
        samples: samples.into_boxed_slice(),
        channels,
        capacity: capacity_frames,
        published: AtomicUsize::new(0),
        released: AtomicUsize::new(0),
        written: AtomicU64::new(0),
        consumed: AtomicU64::new(0),
    });
    Ok((
        PcmProducer {
            shared: Arc::clone(&shared),
            busy: AtomicBool::new(false),
        },
        PcmConsumer {
            shared,
            busy: AtomicBool::new(false),
        },
    ))
}

struct Operation<'a>(&'a AtomicBool);
impl<'a> Operation<'a> {
    fn begin(busy: &'a AtomicBool) -> Option<Self> {
        busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self(busy))
    }
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Shared {
    fn distance(&self, read: usize, written: usize) -> usize {
        (written + self.capacity * 2 - read) % (self.capacity * 2)
    }

    fn spans(&self, position: usize, frames: usize) -> (&[AtomicU32], &[AtomicU32]) {
        let start = position % self.capacity;
        let first = frames.min(self.capacity - start);
        (
            &self.samples[start * self.channels..(start + first) * self.channels],
            &self.samples[..(frames - first) * self.channels],
        )
    }

    fn stats(&self) -> PcmFifoStats {
        let written = self.published.load(Ordering::Acquire);
        let read = self.released.load(Ordering::Acquire);
        let queued = self.distance(read, written);
        PcmFifoStats {
            queued_frames: if queued <= self.capacity { queued } else { 0 },
            written_frames: self.written.load(Ordering::Acquire),
            consumed_frames: self.consumed.load(Ordering::Acquire),
        }
    }
}

impl PcmProducer {
    /// Copies as many complete frames as fit, preserving unread PCM on overflow.
    /// Trailing incomplete input is not consumed. Returns transferred **frames**,
    /// or zero when full, incomplete or concurrently borrowed; never waits.
    #[must_use]
    pub fn write(&self, source: &[f32]) -> usize {
        let Some(_operation) = Operation::begin(&self.busy) else {
            return 0;
        };
        let queue = &self.shared;
        let written = queue.published.load(Ordering::Relaxed);
        let read = queue.released.load(Ordering::Acquire);
        let used = queue.distance(read, written);
        let frames = (source.len() / queue.channels).min(queue.capacity.saturating_sub(used));
        let (first, second) = queue.spans(written, frames);
        for (sample, slot) in source.iter().zip(first.iter().chain(second)) {
            slot.store(sample.to_bits(), Ordering::Relaxed);
        }
        queue.written.fetch_add(frames as u64, Ordering::Release);
        queue
            .published
            .store((written + frames) % (queue.capacity * 2), Ordering::Release);
        frames
    }

    /// Observes occupancy and actual consumed-frame feedback without borrowing PCM.
    #[must_use]
    pub fn stats(&self) -> PcmFifoStats {
        self.shared.stats()
    }
}

impl PcmConsumer {
    /// Reads available complete frames and silences the unfilled destination suffix.
    /// Returns transferred **frames**, or zero when empty, incomplete or concurrently
    /// borrowed. A failed call still silences the whole destination and never waits.
    #[must_use]
    pub fn read(&self, destination: &mut [f32]) -> usize {
        let Some(_operation) = Operation::begin(&self.busy) else {
            destination.fill(0.0);
            return 0;
        };
        let queue = &self.shared;
        let read = queue.released.load(Ordering::Relaxed);
        let written = queue.published.load(Ordering::Acquire);
        let available = queue.distance(read, written);
        let frames = if available <= queue.capacity {
            (destination.len() / queue.channels).min(available)
        } else {
            0
        };
        let (first, second) = queue.spans(read, frames);
        for (slot, sample) in first.iter().chain(second).zip(destination.iter_mut()) {
            *sample = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        destination[frames * queue.channels..].fill(0.0);
        queue
            .released
            .store((read + frames) % (queue.capacity * 2), Ordering::Release);
        queue.consumed.fetch_add(frames as u64, Ordering::Release);
        frames
    }

    /// Retires currently published frames without advancing the consumer's clock.
    /// Returns false only when another read/discard already owns this consumer role.
    #[must_use]
    pub fn discard(&self) -> bool {
        let Some(_operation) = Operation::begin(&self.busy) else {
            return false;
        };
        self.shared.released.store(
            self.shared.published.load(Ordering::Acquire),
            Ordering::Release,
        );
        true
    }

    /// Observes available frames and cumulative frame counters without consuming PCM.
    #[must_use]
    pub fn stats(&self) -> PcmFifoStats {
        self.shared.stats()
    }
}

#[cfg(test)]
mod tests;
