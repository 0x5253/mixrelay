//! Engine-owned retention of drained client PCM, at native rate and native coordinates.
#[cfg(test)]
mod tests;

use mixrelay_macos_transport::client_pcm::{ClientPcmClock, MixWindow};
use std::collections::VecDeque;

/// Store size in native frames: four seconds at the highest product rate, the same
/// retention every other capture transport keeps.
const FRAMES: u64 = mixrelay_audio_io::retention::RING_FRAMES as u64;
/// Separately retained timelines or gaps. Each discontinuity opens a run; past this
/// many, the oldest run is dropped whole. Steady state holds one.
const MAX_RUNS: usize = 16;

/// The client timeline identity that frame coordinates are relative to.
pub(super) type Timeline = ([u8; 16], u64, u64, u32);

pub(super) fn timeline(clock: ClientPcmClock) -> Timeline {
    (clock.incarnation, clock.epoch, clock.anchor, clock.rate)
}

/// Native frames contiguous on one timeline, starting at store position `start`.
#[derive(Clone, Copy, Debug)]
struct Run {
    clock: ClientPcmClock,
    first: u64,
    start: u64,
    restart: bool,
}

/// The oldest unconverted frames that are contiguous on one native timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Chunk {
    pub(super) clock: ClientPcmClock,
    /// Native frame of the first sample on `clock`'s timeline.
    pub(super) first: u64,
    pub(super) frames: usize,
    /// The converter must start a new stream here: the first run, a new timeline,
    /// or frames missing before it.
    pub(super) restart: bool,
}

/// One endpoint's drained, not yet converted PCM. Positions grow monotonically and
/// map onto a preallocated ring, so retention never allocates on RT.
pub(super) struct Backlog {
    samples: Vec<f32>,
    channels: usize,
    runs: VecDeque<Run>,
    /// Coordinate after the last retained frame. It outlives an emptied store so the
    /// next window continues the converter's stream without a restart.
    tail: Option<(Timeline, u64)>,
    read: u64,
    written: u64,
    dropped: u64,
}

impl Backlog {
    /// Reserves and pre-faults the whole store on control, so no page is first touched
    /// on RT whatever the allocator does with a zeroed reservation.
    /// # Errors
    /// Returns the reservation failure before any pump state exists.
    pub(super) fn new(channels: usize) -> std::io::Result<Self> {
        let samples = usize::try_from(FRAMES)
            .ok()
            .and_then(|frames| frames.checked_mul(channels))
            .ok_or(std::io::ErrorKind::InvalidInput)?;
        let mut samples =
            mixrelay_audio_io::pcm_buffer::zeroed(samples).map_err(std::io::Error::other)?;
        mixrelay_pcm_ring::prefault_pcm(&mut samples);
        let mut runs = VecDeque::new();
        runs.try_reserve_exact(MAX_RUNS)
            .map_err(std::io::Error::other)?;
        // Run metadata must also have writable pages before the first RT push.
        let empty = Run {
            clock: ClientPcmClock {
                incarnation: [0; 16],
                epoch: 0,
                anchor: 0,
                rate: 0,
                running: false,
                first: None,
                through: 0,
            },
            first: 0,
            start: 0,
            restart: false,
        };
        runs.resize(MAX_RUNS, empty);
        for run in &mut runs {
            // SAFETY: every reserved run is initialized and exclusively owned;
            // volatile stores prevent removing these pre-fault writes before clear.
            unsafe { std::ptr::write_volatile(run, empty) };
        }
        runs.clear();
        Ok(Self {
            samples,
            channels,
            runs,
            tail: None,
            read: 0,
            written: 0,
            dropped: 0,
        })
    }

    /// Unconverted frames.
    pub(super) fn frames(&self) -> u64 {
        self.written - self.read
    }

    /// Cumulative frames dropped because retention or its run table was full.
    pub(super) fn dropped_frames(&self) -> u64 {
        self.dropped
    }

    /// Retains one mixed window. Retention keeps the newest PCM: a full store first
    /// drops its oldest frames and marks the gap, keeping every earlier run intact.
    pub(super) fn push(&mut self, window: MixWindow, samples: &[f32]) {
        let frames = u64::from(window.frames);
        let identity = timeline(window.clock);
        let continues = !window.discontinuity && self.tail == Some((identity, window.first));
        self.discard(
            (self.written + frames).saturating_sub(self.read + FRAMES),
            true,
        );
        if !continues || self.runs.is_empty() {
            if self.read == self.written {
                self.runs.clear();
            } else if self
                .runs
                .back()
                .is_some_and(|last| last.start == self.written)
            {
                // A run emptied by discard has nothing left to keep.
                self.runs.pop_back();
            }
            if self.runs.len() == MAX_RUNS {
                let end = self.runs[1].start;
                self.discard(end - self.read, true);
            }
            self.runs.push_back(Run {
                clock: window.clock,
                first: window.first,
                start: self.written,
                restart: !continues,
            });
        }
        let slot = self.slot(self.written);
        let (head, wrapped) = samples.split_at(samples.len().min(self.samples.len() - slot));
        self.samples[slot..slot + head.len()].copy_from_slice(head);
        self.samples[..wrapped.len()].copy_from_slice(wrapped);
        self.written += frames;
        self.tail = Some((identity, window.first + frames));
    }

    /// At most `frames` of the oldest retained run, without consuming them.
    pub(super) fn peek(&self, frames: usize) -> Option<Chunk> {
        let head = self.runs.front()?;
        let end = self.runs.get(1).map_or(self.written, |next| next.start);
        let frames = usize::try_from(end - self.read).map_or(frames, |run| run.min(frames));
        (frames != 0).then_some(Chunk {
            clock: head.clock,
            first: head.first + (self.read - head.start),
            frames,
            restart: head.restart,
        })
    }

    /// Copies the oldest unconverted frames into interleaved `output`, which holds at
    /// most the peeked chunk.
    pub(super) fn copy(&self, output: &mut [f32]) {
        let slot = self.slot(self.read);
        let (head, wrapped) = output.split_at_mut(output.len().min(self.samples.len() - slot));
        head.copy_from_slice(&self.samples[slot..slot + head.len()]);
        wrapped.copy_from_slice(&self.samples[..wrapped.len()]);
    }

    /// The converter started a new stream at the head run.
    pub(super) fn restarted(&mut self) {
        if let Some(head) = self.runs.front_mut() {
            head.restart = false;
        }
    }

    pub(super) fn consume(&mut self, frames: usize) {
        self.read = (self.read + frames as u64).min(self.written);
        while self.runs.len() > 1 && self.runs[1].start <= self.read {
            self.runs.pop_front();
        }
    }

    /// Keeps only the newest `frames` of the newest run, for a stream that must not
    /// start behind live PCM. The trimmed head is policy, not loss, and the stream
    /// restarts after it.
    pub(super) fn keep_newest(&mut self, frames: u64) {
        let run = self.runs.back().map_or(self.written, |run| run.start);
        let first = self.written.saturating_sub(frames).max(run).max(self.read);
        self.discard(first - self.read, false);
    }

    /// Ends retention of a stopped timeline; returns the unconverted frames it held.
    pub(super) fn clear(&mut self) -> u64 {
        let frames = self.frames();
        self.read = self.written;
        self.runs.clear();
        self.tail = None;
        frames
    }

    /// Drops the oldest unconverted frames; the run continuing after them restarts.
    fn discard(&mut self, frames: u64, lost: bool) {
        if frames == 0 {
            return;
        }
        self.read += frames;
        if lost {
            self.dropped += frames;
        }
        while self.runs.len() > 1 && self.runs[1].start <= self.read {
            self.runs.pop_front();
        }
        if let Some(head) = self.runs.front_mut()
            && head.start < self.read
        {
            head.first += self.read - head.start;
            head.start = self.read;
            head.restart = true;
        }
    }

    fn slot(&self, position: u64) -> usize {
        // The remainder is below FRAMES, which fits usize by construction.
        usize::try_from(position % FRAMES).unwrap_or(0) * self.channels
    }
}
