//! Native master controls for OS playback, independent of graph-local gain.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub(crate) const MIN_DECIBELS: f32 = -96.0;

pub(crate) struct OutputControl {
    scalar: AtomicU32,
    muted: AtomicBool,
}

impl Default for OutputControl {
    fn default() -> Self {
        Self {
            scalar: AtomicU32::new(1.0_f32.to_bits()),
            muted: AtomicBool::new(false),
        }
    }
}

impl OutputControl {
    pub(crate) fn values(&self) -> (f32, bool) {
        (
            f32::from_bits(self.scalar.load(Ordering::Acquire)),
            self.muted.load(Ordering::Acquire),
        )
    }

    pub(crate) fn set_scalar(&self, scalar: f32) -> bool {
        self.scalar.swap(scalar.to_bits(), Ordering::AcqRel) != scalar.to_bits()
    }

    pub(crate) fn set_muted(&self, muted: bool) -> bool {
        self.muted.swap(muted, Ordering::AcqRel) != muted
    }

    pub(crate) fn apply(&self, samples: &mut [f32]) {
        let (scalar, muted) = self.values();
        if muted || scalar == 0.0 {
            samples.fill(0.0);
        } else if scalar.to_bits() != 1.0_f32.to_bits() {
            for sample in samples {
                *sample *= scalar;
            }
        }
    }
}

pub(crate) fn scalar_to_decibels(scalar: f32) -> Option<f32> {
    if !scalar.is_finite() || !(0.0..=1.0).contains(&scalar) {
        return None;
    }
    // Scalar zero is digital silence; the finite advertised dB range reports
    // its lower limit rather than returning an infinity to CoreAudio clients.
    Some((20.0 * scalar.log10()).max(MIN_DECIBELS))
}

pub(crate) fn decibels_to_scalar(decibels: f32) -> Option<f32> {
    (decibels.is_finite() && (MIN_DECIBELS..=0.0).contains(&decibels))
        .then(|| 10.0_f32.powf(decibels / 20.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_use_amplitude_decibels_and_keep_mute_separate() {
        let control = OutputControl::default();
        assert_eq!(control.values(), (1.0, false));
        assert!(!control.set_scalar(1.0));
        assert!(control.set_scalar(0.5));
        let decibels = scalar_to_decibels(0.5).unwrap();
        assert!((decibels + 6.0206).abs() < 0.0001);
        assert!((decibels_to_scalar(decibels).unwrap() - 0.5).abs() < 0.0001);
        assert_eq!(scalar_to_decibels(0.0), Some(MIN_DECIBELS));
        assert!(control.set_muted(true));
        let mut samples = [0.75, -0.75];
        control.apply(&mut samples);
        assert_eq!(samples.map(f32::to_bits), [0.0_f32; 2].map(f32::to_bits));
        assert_eq!(control.values(), (0.5, true));
        assert!(control.set_muted(false));
        samples = [0.75, -0.75];
        control.apply(&mut samples);
        assert_eq!(
            samples.map(f32::to_bits),
            [0.375_f32, -0.375].map(f32::to_bits)
        );
        control.set_scalar(0.0);
        samples = [f32::NAN, f32::INFINITY];
        control.apply(&mut samples);
        assert_eq!(samples.map(f32::to_bits), [0.0_f32; 2].map(f32::to_bits));
    }

    #[test]
    fn level_conversion_rejects_nonfinite_and_out_of_range_values() {
        for scalar in [-1.0, 1.1, f32::NAN, f32::INFINITY] {
            assert_eq!(scalar_to_decibels(scalar), None);
        }
        for decibels in [-97.0, 1.0, f32::NAN, f32::NEG_INFINITY] {
            assert_eq!(decibels_to_scalar(decibels), None);
        }
        assert_eq!(decibels_to_scalar(0.0), Some(1.0));
    }
}
