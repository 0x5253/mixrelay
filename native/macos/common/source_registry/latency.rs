//! Pure consumer-owned latency policy. All quantities are graph frames. The
//! adapter supplies measured scheduling bursts; no time, atomics or PCM here.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Latency {
    pub epoch: u64,
    pub next: u64,
    pub started: bool,
    pub rebuffering: bool,
    pub target: u32,
    pub consumer_burst: u32,
    burst: u32,
    quantum: u32,
    pub starvation_episodes: u64,
    pub starvation_frames: u64,
    pub rebuffer_frames: u64,
    pub overwrite_skips: u64,
    pub insufficient_headroom: bool,
    /// How a time-addressed reader's clock stands against the frame timestamps.
    pub timing: super::timestamps::Timing,
}

impl Latency {
    /// Complete a still-current PCM copy. Readiness alone cannot end an episode:
    /// an overwrite may race the copy after the watermark was sampled.
    pub(super) fn consumed(mut self, frames: u32) -> Self {
        self.next += u64::from(frames);
        self.rebuffering = false;
        self
    }
}

#[derive(Clone, Copy)]
pub(super) struct Observation {
    pub epoch: u64,
    pub published: u64,
    pub frames: u32,
    pub capacity: u32,
    pub producer_burst: u32,
    pub converter_burst: u32,
    pub drought: u32,
    pub running: bool,
    /// Consecutive graph slices in the same observed scheduling burst.
    pub adjacent: bool,
    /// A concurrent overwrite made a previously ready copy incomplete.
    pub copy_missing: u32,
}

fn requirement(input: Observation, consumer_burst: u32, quantum: u32) -> u32 {
    let producer = if input.producer_burst == 0 {
        512
    } else {
        input.producer_burst
    };
    producer
        .max(consumer_burst)
        .max(input.converter_burst)
        .saturating_add(input.drought)
        .saturating_add(quantum)
}

/// Returns the next state and whether the existing cursor can be read. The
/// caller advances it only after a complete, still-current PCM copy. A target
/// change never rewinds a cursor. Live shrink is deliberately disabled.
pub(super) fn transition(mut state: Latency, input: Observation) -> (Latency, bool) {
    let q = input.frames.max(1);
    if state.epoch != input.epoch {
        state.epoch = input.epoch;
        state.started = false;
        state.rebuffering = false;
        state.target = 0;
        state.consumer_burst = 0;
        state.burst = 0;
        state.quantum = 0;
    }
    state.quantum = state.quantum.max(q);
    let quantum = state.quantum;
    if input.copy_missing == 0 {
        state.burst = if input.adjacent {
            state.burst.saturating_add(q)
        } else {
            q
        };
        state.consumer_burst = state.consumer_burst.max(state.burst);
    }
    let required = requirement(input, state.consumer_burst, quantum);
    let bound = input.capacity / 2;
    let floor = required
        .div_ceil(quantum)
        .saturating_mul(quantum)
        .min(bound);
    state.insufficient_headroom = required > bound;
    if q > bound {
        // An impossible read quantum must not underflow an overwrite anchor or
        // exceed the latency bound. Leave the unread cursor untouched.
        state.target = bound;
        return (state, false);
    }
    if !state.started {
        state.target = floor;
        state.next = 0;
        state.rebuffering = false;
        if input.published < u64::from(state.target.max(q)) {
            return (state, false);
        }
        state.next = input.published - u64::from(state.target.max(q));
        state.started = true;
    }
    if input.published > state.next.saturating_add(u64::from(input.capacity)) {
        let next = input.published - u64::from(state.target.max(q));
        state.overwrite_skips = state.overwrite_skips.saturating_add(next - state.next);
        state.next = next;
        state.rebuffering = false;
    }
    let backlog = input.published.saturating_sub(state.next);
    if !state.rebuffering && (backlog < u64::from(q) || input.copy_missing != 0) {
        if !input.running {
            return (state, false);
        }
        state.starvation_episodes = state.starvation_episodes.saturating_add(1);
        let deficit = u64::from(q)
            .saturating_sub(backlog)
            .max(u64::from(input.copy_missing));
        let growth = quantum
            .max(state.target.div_ceil(2))
            .max(u32::try_from(deficit).unwrap_or(q));
        state.target = state.target.saturating_add(growth).max(floor).min(bound);
        state.rebuffering = true;
    }
    if state.rebuffering && (backlog < u64::from(state.target.max(q)) || input.copy_missing != 0) {
        state.starvation_frames = state.starvation_frames.saturating_add(
            u64::from(q)
                .saturating_sub(backlog)
                .max(u64::from(input.copy_missing)),
        );
        state.rebuffer_frames = state.rebuffer_frames.saturating_add(u64::from(q));
        return (state, false);
    }
    (state, true)
}

#[cfg(test)]
mod tests;
