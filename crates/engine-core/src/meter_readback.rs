//! Publishes revision-bound execution metadata and drains shared peak meters.

use crate::{
    Arc, CompileLimits, EngineGraphController, MeterBinding, MeterSnapshot, NodeMeterSnapshot,
    PROTOCOL_VERSION, PeakMeterBank, execution_control, meter_snapshot,
};

impl EngineGraphController {
    pub(super) fn meter_snapshot_at(&self, now: mixrelay_time::Instant) -> MeterSnapshot {
        self.catalog.inspect(|catalog| {
            let graph_id = catalog
                .and_then(|value| value.as_ref().ok())
                .map_or("session", |catalog| catalog.selected_id());
            let bindings = self.meter_bindings.read();
            let views = self.execution_views.read();
            let revisions = self.exchange.plan_revisions();
            let view = views.get(&revisions.active);
            let applied_revision = view.map_or(revisions.active, |view| {
                revisions.active.max(view.controls.applied_revision())
            });
            let key = meter_snapshot::MeterKey {
                graph_id: graph_id.to_owned(),
                plan_revision: revisions.active,
                submitted_revision: revisions.submitted,
                applied_revision,
                enabled: self.power.enabled(),
            };
            let valid = || {
                self.exchange.plan_revisions() == revisions
                    && view.is_none_or(|view| {
                        revisions.active.max(view.controls.applied_revision()) == applied_revision
                    })
                    && self.power.enabled() == key.enabled
            };
            self.meter_publisher.lock().read(
                now,
                key.clone(),
                |measured| {
                    let nodes = bindings
                        .get(&revisions.active)
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .filter_map(|(index, binding)| {
                            // Only this publisher drains. Hidden graph bindings are also consumed.
                            let peaks = if measured {
                                self.meters
                                    .take_peaks(applied_revision, index, binding.channels())
                            } else {
                                vec![0.0; usize::from(binding.channels())]
                            };
                            let dynamics = (measured && binding.has_dynamics())
                                .then(|| {
                                    self.meters.take_dynamics(
                                        applied_revision,
                                        index,
                                        binding.channels(),
                                    )
                                })
                                .flatten()
                                .map(|reading| mixrelay_control_protocol::DynamicsMeterSnapshot {
                                    input_peaks: reading.input_peaks,
                                    gain_reduction_db: reading.gain_reduction_db,
                                    threshold_db: reading.threshold_db,
                                });
                            let origin = view?.origins.get(binding.node_id())?;
                            (origin.graph_id == graph_id).then(|| NodeMeterSnapshot {
                                node_id: origin.node_id.as_str().to_owned(),
                                port_id: origin
                                    .meter_port
                                    .as_ref()
                                    .unwrap_or(binding.port_id())
                                    .as_str()
                                    .to_owned(),
                                peaks,
                                dynamics,
                            })
                        })
                        .collect();
                    MeterSnapshot {
                        protocol_version: PROTOCOL_VERSION,
                        revision: applied_revision,
                        nodes,
                    }
                },
                valid,
            )
        })
    }

    /// Publishes read-side metadata after native preparation, before the exchange exposes a plan.
    pub(super) fn publish_execution_metadata(
        &self,
        revision: u64,
        bindings: Vec<MeterBinding>,
        view: execution_control::ExecutionView,
    ) {
        let active = self.exchange.plan_revisions().active;
        let mut meters = self.meter_bindings.write();
        let mut views = self.execution_views.write();
        meters.retain(|existing, _| *existing == active);
        meters.insert(revision, bindings);
        views.retain(|existing, _| *existing == active);
        views.insert(revision, view);
    }
}
#[must_use]
pub fn default_peak_meter_bank() -> Arc<PeakMeterBank> {
    let limits = CompileLimits::default();
    Arc::new(PeakMeterBank::new(
        limits.max_nodes.saturating_add(1),
        usize::from(limits.max_channels_per_port),
    ))
}
