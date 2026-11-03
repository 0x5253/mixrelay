//! One control-side publisher owns meter draining; API readers share its immutable window.
use mixrelay_time::Instant;
use std::sync::Arc;

use mixrelay_audio_engine::PeakMeterBank;
use mixrelay_control_protocol::MeterSnapshot;

const WINDOW_NANOS: u128 = 50_000_000;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct MeterKey {
    pub graph_id: String,
    pub plan_revision: u64,
    pub submitted_revision: u64,
    pub applied_revision: u64,
    pub enabled: bool,
}

struct Window {
    index: u128,
    key: MeterKey,
    snapshot: MeterSnapshot,
    measured: bool,
}

pub(super) struct MeterPublisher {
    bank: Arc<PeakMeterBank>,
    origin: Instant,
    window: Option<Window>,
    consumed_sequence: Option<u64>,
}

impl MeterPublisher {
    pub(super) fn new(bank: Arc<PeakMeterBank>, origin: Instant) -> Self {
        Self {
            bank,
            origin,
            window: None,
            consumed_sequence: None,
        }
    }

    /// Windows are fixed to the controller's clock, never extended by reader traffic.
    /// Without readers, the next window drains the accumulated peak since the last
    /// publication. No thread/timer runs on behalf of telemetry.
    pub(super) fn read(
        &mut self,
        now: Instant,
        key: MeterKey,
        mut capture: impl FnMut(bool) -> MeterSnapshot,
        valid: impl Fn() -> bool,
    ) -> MeterSnapshot {
        let requested_index = now.duration_since(self.origin).as_nanos() / WINDOW_NANOS;
        // A request timestamp can precede a lock wait; publication time never regresses.
        let index = self
            .window
            .as_ref()
            .map_or(requested_index, |window| requested_index.max(window.index));
        let bank = &self.bank;
        let publication = bank.publication();
        let fresh = publication.is_some_and(|publication| {
            Some(publication.sequence) != self.consumed_sequence
                && publication.revision == key.applied_revision
        });
        if let Some(window) = &self.window
            && window.index == index
            && window.key == key
            && (window.measured || !fresh)
            && valid()
        {
            return window.snapshot.clone();
        }
        let can_measure = fresh && key.enabled && key.plan_revision == key.submitted_revision;
        let mut snapshot = capture(can_measure);
        let measured = can_measure && bank.publication() == publication && valid();
        if can_measure {
            // Even a rejected capture may have consumed the next in-progress block.
            // Its completion must never be published as valid silence/unity.
            self.consumed_sequence = Some(bank.drain_rejection_floor());
            if !measured {
                snapshot = capture(false);
            }
        } else if !key.enabled {
            // Off -> On without a new callback must not resurrect an undrained peak.
            self.consumed_sequence = Some(bank.drain_rejection_floor());
        }
        self.window = Some(Window {
            index,
            key,
            snapshot: snapshot.clone(),
            measured,
        });
        snapshot
    }
}
