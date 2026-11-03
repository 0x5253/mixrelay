//! Biquad mathematics from the W3C Audio EQ Cookbook; implementation authored for `MixRelay`.
//! <https://www.w3.org/TR/audio-eq-cookbook/> (no external implementation copied).
use std::f64::consts::TAU;

#[derive(Clone, Copy)]
pub(super) enum Shape {
    LowPass,
    HighPass,
    Peak,
}

#[allow(clippy::many_single_char_names)] // The equation names match the Audio EQ Cookbook.
pub(super) fn coefficients(shape: Shape, hz: f32, q: f32, db: f32, rate: u32) -> [f64; 5] {
    let w = TAU * f64::from(hz).min(f64::from(rate) * 0.45) / f64::from(rate);
    let (s, c) = w.sin_cos();
    let a = 10.0_f64.powf(f64::from(db) / 40.0);
    let alpha = s / (2.0 * f64::from(q));
    let (b, d) = match shape {
        Shape::LowPass => (
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        ),
        Shape::HighPass => (
            [1.0_f64.midpoint(c), -(1.0 + c), 1.0_f64.midpoint(c)],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        ),
        Shape::Peak => (
            [1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a],
        ),
    };
    [
        b[0] / d[0],
        b[1] / d[0],
        b[2] / d[0],
        d[1] / d[0],
        d[2] / d[0],
    ]
}

#[allow(clippy::cast_possible_truncation)] // Only the final audio sample is narrowed to the host format.
pub(super) fn sample(value: f32, c: &[f64], z: &mut [f64; 2]) -> f32 {
    // Low cutoffs at high sample rates require retaining the tiny difference
    // between the feedback coefficients. Narrowing either coefficients or the
    // recursive history to f32 changes the response of quiet signals.
    let input = f64::from(value);
    let output = finite(c[0].mul_add(input, z[0]));
    z[0] = history_sample(c[1].mul_add(input, z[1] - c[3] * output));
    z[1] = history_sample(c[2].mul_add(input, -c[4] * output));
    super::finite(output as f32)
}

fn finite(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

fn history_sample(value: f64) -> f64 {
    if value.abs() < 1e-20 {
        0.0
    } else {
        finite(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{Shape, coefficients, sample};

    #[test]
    fn quiet_low_pass_dc_remains_unity_at_low_cutoff_and_high_sample_rates() {
        for rate in [44_100, 48_000, 96_000, 192_000, 384_000] {
            for q in [0.1, std::f32::consts::FRAC_1_SQRT_2, 10.0] {
                let c = coefficients(Shape::LowPass, 20.0, q, 0.0, rate);
                for input in [0.01, 0.1] {
                    let mut history = [0.0; 2];
                    let mut output = 0.0;
                    for _ in 0..rate * 2 {
                        output = sample(input, &c, &mut history);
                    }
                    assert!(
                        (output - input).abs() < input * 0.0001,
                        "LP DC: rate {rate}, Q {q}, input {input}, output {output}"
                    );
                }
            }
        }
    }

    #[test]
    fn high_pass_rejects_quiet_dc_at_low_cutoff_and_high_sample_rates() {
        for rate in [44_100, 48_000, 96_000, 192_000, 384_000] {
            for q in [0.1, std::f32::consts::FRAC_1_SQRT_2, 10.0] {
                let c = coefficients(Shape::HighPass, 20.0, q, 0.0, rate);
                let mut history = [0.0; 2];
                let mut output = 0.0;
                for _ in 0..rate * 2 {
                    output = sample(0.01, &c, &mut history);
                }
                assert!(
                    output.abs() < 0.000_001,
                    "HP DC: rate {rate}, Q {q}, output {output}"
                );
            }
        }
    }

    #[test]
    fn low_cutoff_frequency_response_matches_cookbook_transfer_function() {
        for rate in [192_000, 384_000] {
            for (shape, hz) in [(Shape::LowPass, 1.0), (Shape::HighPass, 100.0)] {
                let c = coefficients(shape, 20.0, 10.0, 0.0, rate);
                let expected = response(c, hz, rate);
                let mut history = [0.0; 2];
                let mut power = 0.0;
                for frame in 0..rate * 3 {
                    let wave =
                        (std::f64::consts::TAU * hz * f64::from(frame) / f64::from(rate)).sin();
                    #[allow(clippy::cast_possible_truncation)]
                    // Test input uses the real host's f32 samples.
                    let input = (0.01 * wave) as f32;
                    let output = f64::from(sample(input, &c, &mut history));
                    if frame >= rate * 2 {
                        power += output * output;
                    }
                }
                let actual = (power / f64::from(rate)).sqrt() / (0.01 / 2.0_f64.sqrt());
                assert!(
                    (actual - expected).abs() < 0.0002,
                    "response: rate {rate}, Hz {hz}, expected {expected}, actual {actual}"
                );
            }
        }
    }

    fn response(c: [f64; 5], hz: f64, rate: u32) -> f64 {
        let omega = std::f64::consts::TAU * hz / f64::from(rate);
        let numerator_real = c[0] + c[1] * omega.cos() + c[2] * (2.0 * omega).cos();
        let numerator_imaginary = -c[1] * omega.sin() - c[2] * (2.0 * omega).sin();
        let denominator_real = 1.0 + c[3] * omega.cos() + c[4] * (2.0 * omega).cos();
        let denominator_imaginary = -c[3] * omega.sin() - c[4] * (2.0 * omega).sin();
        ((numerator_real * numerator_real + numerator_imaginary * numerator_imaginary)
            / (denominator_real * denominator_real + denominator_imaginary * denominator_imaginary))
            .sqrt()
    }
}
