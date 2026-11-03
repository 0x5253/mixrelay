//! Validated content timing uses a common host clock, never device sample-counter subtraction.

/// Measured causal residence plus planned graph and resampler delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLatency {
    /// Planned processing/compensation arrival in graph-rate frames.
    pub arrival_frames: Option<u32>,
    /// Host nanoseconds when this content entered the measured bridge/queue.
    pub bridge_enter_ns: Option<u64>,
    /// Host nanoseconds when that same content left the bridge/queue.
    /// Both timestamps must have the same host clock and exclude graph/SRC delay.
    pub bridge_leave_ns: Option<u64>,
    /// Measured destination scheduling residence, excluding graph and SRC delay.
    pub output_scheduling_ns: Option<u64>,
    /// Validated SRC algorithmic delay in graph-rate frames.
    pub src_frames: Option<u32>,
    pub graph_rate: u32,
    pub destination_rate: u32,
}
impl ContentLatency {
    /// Rounds the complete delay upward once into destination-rate sample frames.
    /// Missing evidence, backwards host time, invalid rates or overflow is unknown.
    #[must_use]
    pub fn frames(self) -> Option<u32> {
        if self.graph_rate == 0 || self.destination_rate == 0 {
            return None;
        }
        let residence = self
            .bridge_leave_ns?
            .checked_sub(self.bridge_enter_ns?)?
            .checked_add(self.output_scheduling_ns?)?;
        let graph_frames = self.arrival_frames?.checked_add(self.src_frames?)?;
        let numerator = u128::from(residence) * u128::from(self.graph_rate)
            + u128::from(graph_frames) * 1_000_000_000;
        let frames = (numerator * u128::from(self.destination_rate))
            .div_ceil(u128::from(self.graph_rate) * 1_000_000_000);
        u32::try_from(frames).ok()
    }
}

/// A control-owner stable value. Unknown invalidates immediately; known changes must
/// exceed the threshold for three consecutive observations in the same direction.
#[derive(Debug, Default)]
pub struct LatencyHysteresis {
    value: Option<u32>,
    candidate: Option<(bool, u8)>,
}
impl LatencyHysteresis {
    /// Returns the retained value; rate/lease/graph changes require `reset` first.
    pub fn observe(&mut self, measured: Option<u32>, threshold: u32) -> Option<u32> {
        let (Some(previous), Some(next)) = (self.value, measured) else {
            self.value = measured;
            self.candidate = None;
            return measured;
        };
        if previous.abs_diff(next) <= threshold {
            self.candidate = None;
            return self.value;
        }
        let direction = next > previous;
        let count = self
            .candidate
            .filter(|(old, _)| *old == direction)
            .map_or(1, |(_, count)| count + 1);
        if count >= 3 {
            self.value = measured;
            self.candidate = None;
        } else {
            self.candidate = Some((direction, count));
        }
        self.value
    }
    /// Invalidates timing on a clock epoch, rate, graph or lease transition.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_host_residence_and_graph_src_delay_once() {
        let latency = ContentLatency {
            arrival_frames: Some(480),
            bridge_enter_ns: Some(10_000_000),
            bridge_leave_ns: Some(12_000_000),
            src_frames: Some(24),
            output_scheduling_ns: Some(0),
            graph_rate: 48_000,
            destination_rate: 44_100,
        };
        assert_eq!(latency.frames(), Some(552));
        for bad in [
            ContentLatency {
                bridge_enter_ns: None,
                ..latency
            },
            ContentLatency {
                bridge_leave_ns: Some(0),
                ..latency
            },
            ContentLatency {
                src_frames: None,
                ..latency
            },
            ContentLatency {
                arrival_frames: None,
                ..latency
            },
            ContentLatency {
                output_scheduling_ns: None,
                ..latency
            },
            ContentLatency {
                graph_rate: 0,
                ..latency
            },
            ContentLatency {
                destination_rate: 0,
                ..latency
            },
            ContentLatency {
                arrival_frames: Some(u32::MAX),
                ..latency
            },
            ContentLatency {
                bridge_leave_ns: Some(u64::MAX),
                ..latency
            },
        ] {
            assert_eq!(bad.frames(), None);
        }
    }
    #[test]
    fn hysteresis_requires_sustained_change_and_invalidates_unknown() {
        let mut stable = LatencyHysteresis::default();
        assert_eq!(stable.observe(Some(100), 4), Some(100));
        for value in [104, 105, 106, 96, 95, 106, 107] {
            assert_eq!(stable.observe(Some(value), 4), Some(100));
        }
        assert_eq!(stable.observe(Some(108), 4), Some(108));
        assert_eq!(stable.observe(None, 4), None);
        assert_eq!(stable.observe(Some(0), 4), Some(0));
        stable.reset();
        assert_eq!(stable.observe(None, 4), None);
    }
}
