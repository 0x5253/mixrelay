//! Hysteresis over one endpoint's measured windows, reset by any clock context change.
use super::arithmetic::threshold;
use mixrelay_render::content_latency::LatencyHysteresis;

pub(super) type ClockContext = (u64, u64, u64, u64, u32, u32, bool, u64);
#[derive(Default)]
pub(super) struct StableEndpoint {
    rate: u32,
    clocks: Vec<ClockContext>,
    hysteresis: LatencyHysteresis,
    value: Option<u32>,
    /// The last logged reason a window could not be measured, so each change logs once.
    pub(super) diagnosis: Option<String>,
}
impl StableEndpoint {
    pub(super) fn apply(
        &mut self,
        rate: u32,
        clocks: &[ClockContext],
        measured: Option<u32>,
        unstable: bool,
    ) -> Option<u32> {
        if self.rate != rate || self.clocks != clocks {
            self.hysteresis.reset();
            self.value = None;
            self.rate = rate;
            self.clocks = clocks.to_vec();
        }
        let threshold = threshold(rate);
        // A disturbed window breaks a candidate streak while retaining the last stable value.
        self.value = self
            .hysteresis
            .observe(if unstable { self.value } else { measured }, threshold);
        self.value
    }
}
impl StableEndpoint {
    /// Logs the reason a window was unusable when it differs from the previous window's, so an
    /// installed `0` (unknown) report can be explained without a line per reporter tick.
    /// Returns the reason it logged.
    pub(super) fn log(
        &mut self,
        id: &mixrelay_audio_io::VirtualEndpointId,
        value: Option<u32>,
        diagnosis: Option<String>,
    ) -> Option<&str> {
        if diagnosis == self.diagnosis {
            return None;
        }
        self.diagnosis = diagnosis;
        let reason = self.diagnosis.as_deref()?;
        tracing::info!(endpoint = %id.as_str(), published = ?value, %reason, "input latency window unusable");
        Some(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_reason_logs_once_and_logs_again_after_recovery() {
        let id = mixrelay_audio_io::VirtualEndpointId::new("device".into());
        let mut endpoint = StableEndpoint::default();
        let reason = || Some("device 1: binding faults advanced".to_owned());
        assert_eq!(
            endpoint.log(&id, None, reason()),
            Some("device 1: binding faults advanced")
        );
        assert_eq!(endpoint.log(&id, None, reason()), None);
        assert_eq!(endpoint.log(&id, Some(10), None), None);
        assert_eq!(
            endpoint.log(&id, Some(10), reason()),
            Some("device 1: binding faults advanced")
        );
        assert_eq!(
            endpoint.log(&id, Some(10), Some("arrival read contended".into())),
            Some("arrival read contended")
        );
    }

    #[test]
    fn bad_windows_retain_stable_and_break_streaks_context_changes_reset() {
        let mut endpoint = StableEndpoint::default();
        let clocks = [(1, 2, 3, 4, 48_000, 48_000, false, 5)];
        assert_eq!(endpoint.apply(48_000, &clocks, Some(100), false), Some(100));
        assert_eq!(endpoint.apply(48_000, &clocks, Some(200), false), Some(100));
        assert_eq!(endpoint.apply(48_000, &clocks, None, true), Some(100));
        for _ in 0..2 {
            assert_eq!(endpoint.apply(48_000, &clocks, Some(200), false), Some(100));
        }
        assert_eq!(endpoint.apply(48_000, &clocks, Some(200), false), Some(200));
        for field in 0..8 {
            let mut changed = clocks;
            match field {
                0 => changed[0].0 += 1,
                1 => changed[0].1 += 1,
                2 => changed[0].2 += 1,
                3 => changed[0].3 += 1,
                4 => changed[0].4 += 1,
                5 => changed[0].5 += 1,
                6 => changed[0].6 = true,
                _ => changed[0].7 += 1,
            }
            assert_eq!(endpoint.apply(48_000, &changed, Some(300), true), None);
            assert_eq!(endpoint.apply(48_000, &clocks, Some(100), false), Some(100));
        }
        assert_eq!(endpoint.apply(44_100, &clocks, None, true), None);
        assert_eq!(endpoint.apply(44_100, &clocks, Some(100), false), Some(100));
        assert_eq!(endpoint.apply(44_100, &clocks, None, false), None);
    }
}
