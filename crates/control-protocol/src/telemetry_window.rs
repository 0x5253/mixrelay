//! Interval reductions for undelivered control observations.
use crate::{ControlTelemetry, MeterTelemetry};

impl ControlTelemetry {
    /// Coalesces meters by interval maximum and state by latest complete replacement.
    pub fn absorb(&mut self, newer: Self) {
        let state = newer.state.clone().or_else(|| {
            (self.engine_incarnation == newer.engine_incarnation)
                .then(|| self.state.clone())
                .flatten()
        });
        let mut meters = MeterTelemetry {
            engine_incarnation: self.engine_incarnation.clone(),
            sequence: self.sequence,
            meters: self.meters.clone(),
        };
        meters.absorb(MeterTelemetry {
            engine_incarnation: newer.engine_incarnation,
            sequence: newer.sequence,
            meters: newer.meters,
        });
        *self = Self {
            engine_incarnation: meters.engine_incarnation,
            sequence: meters.sequence,
            meters: meters.meters,
            state,
        };
    }
}

impl MeterTelemetry {
    /// Folds a newer window into this undelivered one, so a slow subscriber keeps the
    /// latest state and the interval maximum instead of a queue. Peaks and gain reduction
    /// take the maximum per binding; everything else follows the newer window. A window
    /// from another engine or plan revision replaces this one, since its bindings differ.
    pub fn absorb(&mut self, newer: Self) {
        if newer.engine_incarnation != self.engine_incarnation
            || newer.meters.protocol_version != self.meters.protocol_version
            || newer.meters.revision != self.meters.revision
        {
            *self = newer;
            return;
        }
        let mut older = std::mem::take(&mut self.meters.nodes);
        self.sequence = newer.sequence;
        self.meters = newer.meters;
        for node in &mut self.meters.nodes {
            let Some(index) = older
                .iter()
                .position(|old| old.node_id == node.node_id && old.port_id == node.port_id)
            else {
                continue;
            };
            let old = older.swap_remove(index);
            max_peaks(&mut node.peaks, &old.peaks);
            if let (Some(dynamics), Some(old)) = (&mut node.dynamics, old.dynamics) {
                max_peaks(&mut dynamics.input_peaks, &old.input_peaks);
                dynamics.gain_reduction_db = dynamics.gain_reduction_db.max(old.gain_reduction_db);
            }
        }
    }
}

/// Element-wise maximum when both windows describe the same channels.
fn max_peaks(peaks: &mut [f32], older: &[f32]) {
    if peaks.len() == older.len() {
        for (peak, old) in peaks.iter_mut().zip(older) {
            *peak = peak.max(*old);
        }
    }
}
