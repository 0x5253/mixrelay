//! Exact callback-duration conversion between native and render frame units.
//! This is scheduling arithmetic, not PCM resampling or clock drift correction.
use crate::process::{ClockDomain, CycleError, ProcessCycle};

pub struct FrameBudgetMapper {
    output_rate: u32,
    previous: Option<(ClockDomain, u32, u64)>,
    remainder: u64,
    position: u64,
    epoch: u64,
    pending_discontinuity: bool,
}

impl FrameBudgetMapper {
    /// # Errors
    /// A zero output rate cannot define a frame timeline.
    pub fn new(output_rate: u32) -> Result<Self, CycleError> {
        if output_rate == 0 {
            return Err(CycleError::InvalidFormat);
        }
        Ok(Self {
            output_rate,
            previous: None,
            remainder: 0,
            position: 0,
            epoch: 0,
            pending_discontinuity: false,
        })
    }

    /// Carries the fractional frame remainder across contiguous native callbacks.
    /// Clock/rate changes, gaps, and explicit discontinuities reset the fraction.
    /// A returned zero-frame cycle is valid: the caller skips rendering until
    /// subsequent callbacks accumulate a complete output frame.
    /// # Errors
    /// Rejects invalid input or arithmetic overflow without changing any state.
    pub fn map(&mut self, native: ProcessCycle) -> Result<ProcessCycle, CycleError> {
        let invalid = CycleError::InvalidFormat;
        if native.sample_rate == 0 || native.frames == 0 {
            return Err(invalid);
        }
        let end = native
            .first_frame
            .checked_add(u64::from(native.frames))
            .ok_or(invalid)?;
        let discontinuity = native.discontinuity
            || self.previous != Some((native.clock, native.sample_rate, native.first_frame));
        let remainder = if discontinuity { 0 } else { self.remainder };
        let numerator = u64::from(native.frames)
            .checked_mul(u64::from(self.output_rate))
            .and_then(|value| value.checked_add(remainder))
            .ok_or(invalid)?;
        let frames =
            u32::try_from(numerator / u64::from(native.sample_rate)).map_err(|_| invalid)?;
        let next_position = self
            .position
            .checked_add(u64::from(frames))
            .ok_or(invalid)?;
        if let Some(time) = native.monotonic_ns {
            time.checked_add(u64::from(frames) * 1_000_000_000 / u64::from(self.output_rate))
                .ok_or(invalid)?;
        }
        let epoch = if self.previous.is_none() {
            native.clock.epoch
        } else if discontinuity {
            self.epoch.wrapping_add(1)
        } else {
            self.epoch
        };
        let cycle = ProcessCycle {
            clock: ClockDomain {
                epoch,
                ..native.clock
            },
            sample_rate: self.output_rate,
            frames,
            first_frame: self.position,
            discontinuity: discontinuity || self.pending_discontinuity,
            ..native
        };
        self.position = next_position;
        self.epoch = epoch;
        self.remainder = numerator % u64::from(native.sample_rate);
        self.previous = Some((native.clock, native.sample_rate, end));
        self.pending_discontinuity = cycle.discontinuity && frames == 0;
        Ok(cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ClockAuthority;
    fn cycle(rate: u32, frames: u32, first_frame: u64) -> ProcessCycle {
        ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::AudioServer,
                id: 7,
                epoch: 1,
            },
            sample_rate: rate,
            frames,
            first_frame,
            monotonic_ns: None,
            discontinuity: false,
        }
    }
    #[test]
    fn exact_fraction_property_across_rates_and_variable_quantums() {
        for input in [8_000, 44_100, 48_000, 96_000, 192_000] {
            for output in [8_000, 44_100, 48_000, 96_000, 192_000] {
                let mut mapper = FrameBudgetMapper::new(output).unwrap();
                let mut source = 0;
                let mut rendered = 0;
                let mut seed = 19_u32;
                for _ in 0..10_000 {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let frames = seed % 2048 + 1;
                    let result = mapper.map(cycle(input, frames, source)).unwrap();
                    assert_eq!(result.first_frame, rendered);
                    source += u64::from(frames);
                    rendered += u64::from(result.frames);
                    assert_eq!(rendered, source * u64::from(output) / u64::from(input));
                }
            }
        }
    }
    #[test]
    fn overflow_is_transactional_and_discontinuity_resets_fraction() {
        let mut mapper = FrameBudgetMapper::new(48_000).unwrap();
        let first = mapper.map(cycle(44_100, 128, 0)).unwrap();
        assert!(mapper.map(cycle(1, u32::MAX, 128)).is_err());
        assert!(mapper.map(cycle(48_000, 128, u64::MAX)).is_err());
        let next = mapper.map(cycle(44_100, 128, 128)).unwrap();
        assert!(!next.discontinuity);
        assert_eq!(first.frames + next.frames, 256 * 48_000 / 44_100);
        let gap = mapper.map(cycle(44_100, 128, 999)).unwrap();
        assert_eq!(gap.frames, first.frames);
        assert!(gap.discontinuity);
        assert_eq!(gap.clock.epoch, next.clock.epoch + 1);
        let mut changed = cycle(44_100, 128, 1127);
        changed.clock.epoch += 1;
        assert!(mapper.map(changed).unwrap().discontinuity);
    }

    #[test]
    fn skipped_zero_frames_preserve_discontinuity_until_rendered() {
        for change in 0..3 {
            let mut mapper = FrameBudgetMapper::new(8_000).unwrap();
            mapper.map(cycle(192_000, 24, 0)).unwrap();
            let mut native = cycle(192_000, 1, 24);
            match change {
                0 => native.first_frame = 1000,
                1 => native.sample_rate = 96_000,
                _ => native.clock.id += 1,
            }
            let zero = mapper.map(native).unwrap();
            assert_eq!(zero.frames, 0);
            assert!(zero.discontinuity);
            loop {
                native.first_frame += 1;
                let next = mapper.map(native).unwrap();
                assert!(next.discontinuity);
                assert_eq!(next.clock.epoch, zero.clock.epoch);
                if next.frames != 0 {
                    break;
                }
            }
            native.frames = 24;
            native.first_frame += 1;
            assert!(!mapper.map(native).unwrap().discontinuity);
        }
    }
}
