//! Shared scalar conversions for prepared and live DSP controls.

pub(crate) fn db_to_linear(decibels: f32) -> f32 {
    10.0_f32.powf(decibels / 20.0)
}

pub(crate) fn smoothing_coefficient(samples: f32) -> f32 {
    (-1.0 / samples).exp()
}
