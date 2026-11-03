//! Phase-independent continuity evidence for a known, non-silent sine wave.
use serde::Serialize;

/// Measurement over one channel, including boundaries between callback blocks.
/// A low recurrence residual alone is not proof of correct gain, channel routing,
/// latency, or native device behavior. Those require separate assertions.
/// A whole-period deletion/repetition can preserve the sine exactly. Use frame
/// sequence evidence as well when validating lossless, unconverted transport.
#[derive(Debug, Clone, Serialize)]
pub struct ToneContinuity {
    /// Number of actual samples inspected, not a nominal capture duration.
    pub sample_count: usize,
    /// Whether every sample was finite.
    pub finite: bool,
    /// Maximum absolute error in the sine's second-order recurrence.
    pub max_residual: f64,
    /// Zero-based third sample of the largest recurrence error, when measurable.
    pub max_residual_at: Option<usize>,
    /// Longest consecutive sequence of exact zeros.
    pub longest_zero_run: usize,
    /// Largest absolute sample; useful for a separate known-tone level assertion.
    pub peak: f64,
}

impl ToneContinuity {
    /// Measures samples in time order at `sample_rate`, for a tone at `frequency_hz`.
    /// Interleaved callers must select one channel without dropping time samples.
    /// Returns `None` for an invalid rate or frequency outside `(0, Nyquist)`.
    /// Nonfinite PCM remains in the report as a failed measurement, not skipped.
    pub fn measure(
        samples: impl IntoIterator<Item = f32>,
        sample_rate: u32,
        frequency_hz: f64,
    ) -> Option<Self> {
        if sample_rate == 0
            || !frequency_hz.is_finite()
            || frequency_hz <= 0.0
            || frequency_hz >= f64::from(sample_rate) / 2.0
        {
            return None;
        }
        let coefficient =
            2.0 * (std::f64::consts::TAU * frequency_hz / f64::from(sample_rate)).cos();
        let mut report = Self {
            sample_count: 0,
            finite: true,
            max_residual: 0.0,
            max_residual_at: None,
            longest_zero_run: 0,
            peak: 0.0,
        };
        let (mut previous, mut older, mut zero_run) = (0.0, 0.0, 0);
        for sample in samples {
            let sample = f64::from(sample);
            report.finite &= sample.is_finite();
            if report.sample_count >= 2 {
                let residual = (sample - coefficient * previous + older).abs();
                if report.max_residual_at.is_none() || residual > report.max_residual {
                    report.max_residual = residual;
                    report.max_residual_at = Some(report.sample_count);
                }
            }
            zero_run = if sample == 0.0 { zero_run + 1 } else { 0 };
            report.longest_zero_run = report.longest_zero_run.max(zero_run);
            report.peak = report.peak.max(sample.abs());
            older = previous;
            previous = sample;
            report.sample_count += 1;
        }
        Some(report)
    }

    /// Applies the existing 0.125-peak fixture continuity limit: residual at most
    /// 0.001, all finite, at least three samples and no consecutive exact zeros.
    /// This is a discontinuity gate, not a complete spectral/level quality test.
    #[must_use]
    pub fn intact(&self) -> bool {
        self.sample_count >= 3
            && self.finite
            && self.max_residual <= 0.001
            && self.longest_zero_run <= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32) -> Vec<f32> {
        let period = u16::try_from(rate / 100).unwrap();
        (0..rate)
            .map(|frame| {
                let phase = u16::try_from(frame * 10 % u32::from(period)).unwrap();
                0.125 * (std::f32::consts::TAU * f32::from(phase) / f32::from(period)).sin()
            })
            .collect()
    }

    #[test]
    fn rejects_missing_repeated_silent_and_nonfinite_samples() {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let original = tone(rate);
            let measure = |samples: &[f32]| {
                ToneContinuity::measure(samples.iter().copied(), rate, 1000.0).unwrap()
            };
            let clean = measure(&original);
            assert!(clean.intact(), "{rate}: {clean:?}");
            assert!((clean.peak - 0.125).abs() < 0.000_01);
            assert!(!measure(&[]).intact());
            assert!(!measure(&[0.0; 48]).intact());
            for repeated in [false, true] {
                let mut changed = original.clone();
                if repeated {
                    changed.insert(1234, changed[1234]);
                } else {
                    changed.remove(1234);
                }
                let broken = measure(&changed);
                assert!(
                    !broken.intact(),
                    "rate={rate} repeated={repeated}: {broken:?}"
                );
                assert!((1234..=1236).contains(&broken.max_residual_at.unwrap()));
            }
            let mut gap = original.clone();
            gap[2345..2416].fill(0.0);
            assert_eq!(measure(&gap).longest_zero_run, 71);
            assert!(!measure(&gap).intact());
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut invalid = original.clone();
                invalid[3456] = value;
                assert!(!measure(&invalid).intact());
            }
        }
    }

    #[test]
    fn block_iteration_preserves_boundary_faults_and_format_validation() {
        let mut samples = tone(48_000);
        samples[128] = samples[127];
        let report =
            ToneContinuity::measure(samples.chunks(128).flatten().copied(), 48_000, 1000.0)
                .unwrap();
        assert!(!report.intact());
        assert_eq!(report.sample_count, 48_000);
        for (rate, hz) in [
            (0, 1000.0),
            (48_000, 0.0),
            (48_000, -1.0),
            (48_000, 24_000.0),
            (48_000, f64::NAN),
        ] {
            assert!(ToneContinuity::measure([], rate, hz).is_none());
        }
    }

    #[test]
    fn whole_period_loss_requires_frame_identity_not_a_sine_only_gate() {
        let mut samples = tone(48_000);
        samples.drain(960..1008);
        let report = ToneContinuity::measure(samples, 48_000, 1000.0).unwrap();
        assert!(
            report.intact(),
            "a periodic tone cannot prove frame conservation"
        );
        let markers = (0..48_000)
            .filter(|frame| !(960..1008).contains(frame))
            .map(crate::frame_sequence::encode);
        assert!(!crate::frame_sequence::SequenceContinuity::measure(markers).intact());
    }
}
