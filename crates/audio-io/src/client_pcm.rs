//! Client mixing over retained queue observations, without native mapping ownership.
/// Device sample zero and rate come from the same native clock as HAL timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientPcmClock {
    pub incarnation: [u8; 16],
    pub epoch: u64,
    pub anchor: u64,
    pub rate: u32,
    pub running: bool,
    pub first: Option<u64>,
    pub through: u64,
}

/// Read-only client queue observations and epoch-checked PCM access.
/// Implementations retain prepared storage; calls never allocate, lock or block.
pub trait ClientQueueRead {
    fn endpoint(&self) -> [u8; 16];
    fn channels(&self) -> u32;
    fn capture_channels(&self) -> u32;
    fn capacity(&self) -> u32;
    fn clock(&self) -> Option<ClientPcmClock>;
    /// Fills the supplied interleaved output and per-frame coverage for one clock.
    /// Returns false when the epoch, incarnation or membership changed during the read.
    fn read(
        &self,
        clock: ClientPcmClock,
        first: u64,
        output: &mut [f32],
        coverage: &mut [bool],
    ) -> bool;
}

/// Whether a currently running observation still belongs to the requested timeline.
#[must_use]
pub fn same_clock(current: Option<ClientPcmClock>, expected: ClientPcmClock) -> bool {
    current.is_some_and(|current| {
        current.running
            && current.incarnation == expected.incarnation
            && current.epoch == expected.epoch
            && current.anchor == expected.anchor
            && current.rate == expected.rate
    })
}

/// One endpoint's monotonic consumer; it never advances because another reader did.
#[derive(Debug, Default)]
pub struct ClientPcmMixer {
    timeline: Option<([u8; 16], u64, u64, u32)>,
    next: u64,
    discontinuity: bool,
    skipped: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct MixWindow {
    pub clock: ClientPcmClock,
    pub first: u64,
    pub frames: u32,
    pub discontinuity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixError {
    Geometry,
    Changed,
}
impl std::fmt::Display for MixError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Geometry => "invalid virtual PCM mix geometry",
            Self::Changed => "virtual PCM clock or membership changed",
        })
    }
}
impl std::error::Error for MixError {}

impl ClientPcmMixer {
    /// Selects published PCM no later than the current native playhead. A stalled
    /// client contributes silence at its unpublished positions, without holding peers.
    /// `playhead` projects the supplied coherent clock into an eligible native-frame
    /// frontier. A missing projection leaves the cursor unchanged.
    pub fn window<Q: ClientQueueRead>(
        &mut self,
        queues: &[Q],
        playhead: impl Fn(ClientPcmClock) -> Option<u64>,
    ) -> Option<MixWindow> {
        let mut clocks = queues
            .iter()
            .filter_map(ClientQueueRead::clock)
            .filter(|clock| clock.running && clock.first.is_some());
        let mut clock = clocks.next()?;
        let mut first = clock.first?;
        for candidate in clocks {
            if (
                candidate.incarnation,
                candidate.epoch,
                candidate.anchor,
                candidate.rate,
            ) != (clock.incarnation, clock.epoch, clock.anchor, clock.rate)
            {
                return None;
            }
            first = first.min(candidate.first?);
            clock.through = clock.through.max(candidate.through);
        }
        let playhead = playhead(clock)?;
        let published = clock.through;
        clock.through = clock.through.min(playhead);
        let capacity = u64::from(queues.iter().map(ClientQueueRead::capacity).min()?);
        let oldest = first.max(published.saturating_sub(capacity));
        let timeline = (clock.incarnation, clock.epoch, clock.anchor, clock.rate);
        let current = self.is_current(clock);
        let discontinuity = !current || self.next < oldest;
        if discontinuity {
            // Published frames the queue no longer holds: an overrun behind this cursor,
            // or a timeline joined after its first frames were overwritten.
            let from = if current { self.next } else { first };
            self.skipped = self.skipped.saturating_add(oldest.saturating_sub(from));
            self.timeline = Some(timeline);
            self.next = oldest;
            self.discontinuity = true;
        }
        let frames = u32::try_from(clock.through.saturating_sub(self.next).min(128)).ok()?;
        (frames != 0).then_some(MixWindow {
            clock,
            first: self.next,
            frames,
            discontinuity: self.discontinuity,
        })
    }

    /// Sum each registered client's committed frames once. Unpublished source frames contribute silence.
    /// # Errors
    /// Rejects invalid buffer geometry. Concurrent clock or membership changes
    /// return `Changed`; the caller retains the cursor for a current window.
    pub fn mix<Q: ClientQueueRead>(
        queues: &[Q],
        window: MixWindow,
        output: &mut [f32],
        scratch: &mut [f32],
    ) -> Result<(), MixError> {
        output.fill(0.0);
        let Some(first) = queues.first() else {
            return Err(MixError::Geometry);
        };
        let channels = first.capture_channels() as usize;
        let frames = window.frames as usize;
        if channels == 0 || output.len() != frames * channels || frames > 128 {
            return Err(MixError::Geometry);
        }
        let mut coverage = [false; 128];
        for queue in queues {
            let source_channels = queue.channels() as usize;
            if queue.endpoint() != first.endpoint()
                || queue.capture_channels() as usize != channels
                || source_channels * frames > scratch.len()
            {
                output.fill(0.0);
                return Err(MixError::Geometry);
            }
            let scratch = &mut scratch[..source_channels * frames];
            if !queue.read(window.clock, window.first, scratch, &mut coverage[..frames]) {
                output.fill(0.0);
                return Err(MixError::Changed);
            }
            for (target, source) in output
                .chunks_exact_mut(channels)
                .zip(scratch.chunks_exact(source_channels))
            {
                for (target, source) in target.iter_mut().zip(source) {
                    *target += source;
                }
            }
        }
        if queues
            .iter()
            .any(|queue| !same_clock(queue.clock(), window.clock))
        {
            output.fill(0.0);
            return Err(MixError::Changed);
        }
        Ok(())
    }
    fn is_current(&self, clock: ClientPcmClock) -> bool {
        self.timeline == Some((clock.incarnation, clock.epoch, clock.anchor, clock.rate))
    }

    /// Cumulative published frames the cursor skipped because the queues no longer held them.
    #[must_use]
    pub fn skipped_frames(&self) -> u64 {
        self.skipped
    }

    pub fn commit(&mut self, window: MixWindow) {
        self.next = window.first + u64::from(window.frames);
        self.discontinuity = false;
    }
}

#[cfg(test)]
mod tests;
