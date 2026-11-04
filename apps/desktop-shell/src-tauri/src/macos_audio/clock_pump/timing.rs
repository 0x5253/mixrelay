//! Sample-rate validation and the cadence arithmetic the pump schedules by.
use super::PumpError;
use mixrelay_macos_process_tap::TapStats;
use std::time::Duration;

/// Until the tap's cadence is known, assume a common 512-frame callback at 48 kHz.
pub(super) const DEFAULT_PERIOD: Duration = Duration::from_micros(10_667);

/// Realtime budget for one tap callback's worth of conversion: computation within 40%
/// and completion within 80% of the callback period.
pub(super) fn budget(period: Duration) -> (Duration, Duration, Duration) {
    (period, period * 2 / 5, period * 4 / 5)
}

/// The tap's average callback length so far, bounded to plausible native quanta.
pub(super) fn callback_period(health: &TapStats, rate: u32) -> Duration {
    let frames = health
        .captured_frames
        .checked_div(health.callback_count)
        .unwrap_or(512)
        .clamp(64, 4_096);
    Duration::from_nanos(frames * 1_000_000_000 / u64::from(rate.max(1)))
}

/// Accepts integral native rates from 8 kHz to 192 kHz.
pub(super) fn checked_rate(rate: f64) -> Result<u32, PumpError> {
    if !rate.is_finite() || !(8_000.0..=192_000.0).contains(&rate) || rate.fract().abs() > 0.001 {
        return Err(PumpError::UnsupportedRate(rate));
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite integral validated sample rate"
    )]
    Ok(rate as u32)
}

/// The graph sample rate, carried in the low half of the transport's format word.
pub(super) fn graph_rate(format_word: u64) -> u32 {
    let [r0, r1, r2, r3, ..] = format_word.to_le_bytes();
    u32::from_le_bytes([r0, r1, r2, r3])
}

/// Rounds native or coalesced callback bursts up to graph frames.
pub(super) fn graph_frames(native_frames: u64, native_rate: u32, graph_rate: u32) -> u32 {
    u32::try_from(
        u128::from(native_frames)
            .saturating_mul(u128::from(graph_rate))
            .div_ceil(u128::from(native_rate.max(1))),
    )
    .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_realtime_budget_follows_the_tap_callback_cadence() {
        let mut health = TapStats {
            channels: 2,
            sample_rate: 48_000.0,
            ..TapStats::default()
        };
        // Before any callback: the 512-frame default.
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(10_666_666)
        );
        health.callback_count = 100;
        health.captured_frames = 12_800;
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(2_666_666)
        );
        // Implausible averages are bounded to native quanta.
        health.captured_frames = 100;
        assert_eq!(
            callback_period(&health, 48_000),
            Duration::from_nanos(1_333_333)
        );
        assert_eq!(
            budget(Duration::from_millis(10)),
            (
                Duration::from_millis(10),
                Duration::from_millis(4),
                Duration::from_millis(8)
            )
        );
    }

    #[test]
    fn native_rates_are_validated_without_assuming_48k() {
        assert_eq!(checked_rate(44_100.0).unwrap(), 44_100);
        assert_eq!(checked_rate(96_000.0).unwrap(), 96_000);
        for rate in [f64::NAN, f64::INFINITY, -1.0, 0.0, 48_000.5, 384_000.0] {
            assert!(checked_rate(rate).is_err());
        }
    }

    #[test]
    fn coalesced_native_bursts_round_up_to_graph_frames() {
        assert_eq!(graph_frames(128, 44_100, 48_000), 140);
        assert_eq!(graph_frames(1024, 96_000, 44_100), 471);
        assert_eq!(graph_frames(u64::MAX, 8000, 192_000), u32::MAX);
        assert_eq!(graph_rate((7_u64 << 32) | 0xac44), 44_100);
    }
}
