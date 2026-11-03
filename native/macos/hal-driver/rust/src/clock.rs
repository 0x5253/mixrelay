//! Nominal host-derived timestamps and the host's matching clock metadata.
use crate::abi::Timestamp;

// Power-of-two timestamp period above the SDK minimum of 10,923 frames. It is the host's
// IO ring span, which bounds a render request and so sizes every client queue.
pub(crate) const TIMESTAMP_PERIOD: u32 = 16_384;
const _: () =
    assert!(TIMESTAMP_PERIOD == mixrelay_macos_transport::client_pcm::MAX_RENDER_SPAN_FRAMES);

pub(crate) fn property(selector: u32) -> Option<u32> {
    match &selector.to_be_bytes() {
        b"ring" => Some(TIMESTAMP_PERIOD),
        // Timestamps already follow nominal host time; an IIR adds another estimate.
        b"clok" => Some(u32::from_be_bytes(*b"raww")),
        b"cstb" => Some(1),
        _ => None,
    }
}

/// Control mutations are excluded from callback readers by the device IO gate.
/// Each device has its own anchor and discontinuity seed, even at equal rates.
#[derive(Clone)]
pub(crate) struct Clock {
    pub rate: u32,
    pub running: bool,
    anchor: u64,
    seed: u64,
    numerator: u32,
    denominator: u32,
}

impl Clock {
    pub(crate) fn transport_epoch(&self) -> (u64, u64) {
        (self.seed, self.anchor)
    }
    pub(crate) fn new(rate: u32, numerator: u32, denominator: u32) -> Option<Self> {
        (crate::formats::SAMPLE_RATES.contains(&rate) && numerator > 0 && denominator > 0)
            .then_some(Self {
                rate,
                running: false,
                anchor: 0,
                seed: 1,
                numerator,
                denominator,
            })
    }

    pub(crate) fn start(&mut self, now: u64) -> bool {
        if self.running || self.seed == u64::MAX {
            return false;
        }
        self.seed += 1;
        self.anchor = now;
        self.running = true;
        true
    }

    pub(crate) fn set_rate(&mut self, rate: u32, now: u64) -> bool {
        if !crate::formats::SAMPLE_RATES.contains(&rate) || self.seed == u64::MAX {
            return false;
        }
        if self.rate != rate {
            self.rate = rate;
            self.seed += 1;
            self.anchor = now;
        }
        true
    }

    pub(crate) fn timestamp(&self, now: u64, output: &mut Timestamp) -> bool {
        if !self.running {
            return false;
        }
        let elapsed = u128::from(now.saturating_sub(self.anchor));
        let numerator = elapsed * u128::from(self.rate) * u128::from(self.numerator);
        let denominator =
            u128::from(TIMESTAMP_PERIOD) * 1_000_000_000 * u128::from(self.denominator);
        let periods = numerator / denominator;
        let frame = periods.saturating_mul(u128::from(TIMESTAMP_PERIOD));
        let ticks = frame
            .saturating_mul(1_000_000_000)
            .saturating_mul(u128::from(self.denominator))
            / (u128::from(self.rate) * u128::from(self.numerator));
        *output = Timestamp {
            sample_time: u64::try_from(frame).unwrap_or(u64::MAX),
            host_time: self
                .anchor
                .saturating_add(u64::try_from(ticks).unwrap_or(u64::MAX)),
            seed: self.seed,
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_clock_rates_and_discontinuities() {
        let mut first = Clock::new(48_000, 1, 1).unwrap();
        let mut second = Clock::new(96_000, 1, 1).unwrap();
        assert!(first.start(100));
        assert!(second.start(100));
        let mut a = Timestamp::default();
        let mut b = Timestamp::default();
        assert!(first.timestamp(1_000_000_100, &mut a));
        assert!(second.timestamp(1_000_000_100, &mut b));
        assert_eq!(a.sample_time, 32_768);
        assert_eq!(b.sample_time, 81_920);
        assert!(first.set_rate(192_000, 1_000_000_100));
        assert!(first.timestamp(1_000_000_100, &mut a));
        assert_eq!(a.sample_time, 0);
        assert!(a.seed > b.seed);
        assert!(first.running);
        assert_eq!(second.rate, 96_000);
    }
    #[test]
    fn invalid_timebases_and_exhaustion_fail_without_wrapping() {
        assert!(Clock::new(48_000, 0, 1).is_none());
        assert!(Clock::new(48_000, 1, 0).is_none());
        assert!(Clock::new(0, 1, 1).is_none());
        assert!(Clock::new(384_000, 1, 1).is_none());
        let mut clock = Clock::new(48_000, 1, 1).unwrap();
        clock.seed = u64::MAX;
        assert!(!clock.start(0));
        assert!(!clock.set_rate(96_000, 0));
        assert_eq!(clock.seed, u64::MAX);
    }

    #[test]
    fn nontrivial_timebase_has_exact_period_boundaries_and_stable_seed() {
        let mut clock = Clock::new(48_000, 125, 3).unwrap();
        let mut timestamp = Timestamp::default();
        assert!(!clock.timestamp(0, &mut timestamp));
        assert!(clock.start(10));
        assert!(!clock.start(20));
        assert!(clock.timestamp(0, &mut timestamp));
        assert_eq!(
            (timestamp.sample_time, timestamp.host_time, timestamp.seed),
            (0, 10, 2)
        );
        assert!(clock.timestamp(8_192_010, &mut timestamp));
        assert_eq!(
            (timestamp.sample_time, timestamp.host_time),
            (16_384, 8_192_010)
        );
        assert!(clock.set_rate(48_000, 99));
        assert!(clock.timestamp(8_192_010, &mut timestamp));
        assert_eq!((timestamp.host_time, timestamp.seed), (8_192_010, 2));
        clock.running = false;
        assert!(!clock.timestamp(u64::MAX, &mut timestamp));
        assert!(clock.start(100));
        assert!(clock.timestamp(100, &mut timestamp));
        assert_eq!((timestamp.sample_time, timestamp.seed), (0, 3));
    }

    #[test]
    fn extreme_timebase_and_near_maximum_anchor_do_not_wrap() {
        let mut clock = Clock::new(192_000, u32::MAX, 1).unwrap();
        assert!(clock.start(u64::MAX - 10));
        let mut timestamp = Timestamp::default();
        assert!(clock.timestamp(u64::MAX, &mut timestamp));
        assert!(timestamp.host_time >= u64::MAX - 10);
        assert!(clock.timestamp(0, &mut timestamp));
        assert_eq!(timestamp.sample_time, 0);
        assert_eq!(timestamp.host_time, u64::MAX - 10);
        clock.running = false;
        assert!(clock.start(0));
        assert!(clock.timestamp(u64::MAX, &mut timestamp));
        assert_eq!(timestamp.sample_time, u64::MAX);
        assert!(timestamp.host_time > 0);
    }
}
