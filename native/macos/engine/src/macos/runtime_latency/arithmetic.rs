//! Frame arithmetic for published latency, independent of devices and plans.

/// The largest disagreement, in frames, that still counts as one measurement at `rate`.
pub(super) fn threshold(rate: u32) -> u32 {
    32.max(rate.div_ceil(1_000))
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn frames(ns: f64, rate: u32) -> Option<u32> {
    let value = (ns * f64::from(rate) / 1e9).ceil();
    (rate != 0 && ns >= 0.0 && value.is_finite() && value <= f64::from(u32::MAX))
        .then_some(value as u32)
}
pub(super) fn agree(values: impl IntoIterator<Item = Option<u32>>, threshold: u32) -> Option<u32> {
    let mut range: Option<(u32, u32)> = None;
    for value in values {
        let value = value?;
        range = Some(range.map_or((value, value), |(min, max)| {
            (min.min(value), max.max(value))
        }));
    }
    let (min, max) = range?;
    (max - min <= threshold).then_some(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_rounds_once_and_endpoint_bindings_must_agree() {
        assert_eq!(frames(1_000_000.01, 44_100), Some(45));
        for (ns, rate) in [
            (f64::NAN, 48_000),
            (f64::INFINITY, 48_000),
            (-1.0, 48_000),
            (1.0, 0),
            (1e20, 48_000),
        ] {
            assert_eq!(frames(ns, rate), None);
        }
        assert_eq!(agree([Some(100), Some(132)], 32), Some(132));
        assert_eq!(agree([Some(100), Some(133)], 32), None);
        assert_eq!(agree([Some(100), None], 32), None);
        assert_eq!(agree([], 32), None);
        assert_eq!(threshold(48_000), 48);
        assert_eq!(threshold(16_000), 32);
    }
}
