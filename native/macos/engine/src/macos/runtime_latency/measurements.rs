//! Per-endpoint latency windows. These maps and arithmetic live on control; callback taps
//! own no endpoint maps.
#[cfg(test)]
mod tests;

use super::arithmetic::{agree, frames, threshold};
use super::stable_endpoint::{ClockContext, StableEndpoint};
use crate::macos_devices::latency::{Observation, binding};
use mixrelay_render::RenderPlan;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Measurements {
    revision: Option<u64>,
    previous: BTreeMap<String, Observation>,
    pub(super) endpoints: BTreeMap<mixrelay_audio_io::VirtualEndpointId, StableEndpoint>,
    /// The last logged publication summary, so each change logs once.
    publication: Option<String>,
}
impl Measurements {
    /// Logs what this tick sends to the Driver when it differs from the previous tick: whether a
    /// latency plan exists, whether the publication is valid, and each endpoint's frames.
    pub(super) fn log_publication(&mut self, summary: String) -> bool {
        if self.publication.as_ref() == Some(&summary) {
            return false;
        }
        tracing::info!(%summary, "input latency publication");
        self.publication = Some(summary);
        true
    }
}
/// One endpoint's sink bindings in the current window.
struct Sinks<'a> {
    plan: &'a RenderPlan,
    current: &'a BTreeMap<String, Observation>,
    delay: &'a dyn Fn(&str, bool) -> Option<f64>,
    indexes: &'a [usize],
    rate: u32,
}
impl Sinks<'_> {
    /// Destination-rate frames when every sink binding measures and they agree.
    fn measure(&self, arrivals: Option<&[Option<f64>]>) -> Option<u32> {
        agree(
            self.indexes.iter().map(|index| {
                let id = self.plan.sink_bindings()[*index].node_id().as_str();
                if binding(self.current, id, false)?.native_rate != self.rate {
                    return None;
                }
                frames(
                    arrivals?.get(*index).copied().flatten()? + (self.delay)(id, false)?,
                    self.rate,
                )
            }),
            threshold(self.rate),
        )
    }
    /// Names the first sink condition that left a clean window without a measurement.
    fn unmeasured(&self, arrivals: Option<&[Option<f64>]>) -> String {
        for index in self.indexes {
            let id = self.plan.sink_bindings()[*index].node_id().as_str();
            match binding(self.current, id, false) {
                Some(value) if value.native_rate != self.rate => {
                    return format!(
                        "{id}: native rate {} differs from endpoint rate {}",
                        value.native_rate, self.rate
                    );
                }
                None => return format!("{id}: no observation"),
                Some(_) => {}
            }
            if arrivals
                .and_then(|arrivals| arrivals.get(*index).copied().flatten())
                .is_none()
            {
                return format!("{id}: no measured arrival from its sources");
            }
            if (self.delay)(id, false).is_none() {
                return format!("{id}: presentation delay unavailable");
            }
        }
        "sink bindings disagree beyond the threshold".into()
    }
}
/// Planned arrivals from measured sources. A contended read disturbs the window (`true`);
/// an invalid request leaves it unknown. Either way the reason is returned for diagnostics.
fn destination_arrivals(
    plan: &RenderPlan,
    sources: &[Option<f64>],
    rate: u32,
) -> Result<Vec<Option<f64>>, (bool, String)> {
    let threshold_ns = f64::from(threshold(rate)) * 1e9 / f64::from(rate);
    plan.measured_destination_arrivals(sources, threshold_ns)
        .map_err(arrival_failure)
}
fn arrival_failure(error: mixrelay_render::ArrivalError) -> (bool, String) {
    match error {
        // Overlapping the audio thread disturbs this window; it is not unknown.
        mixrelay_render::ArrivalError::Contended => (true, "arrival read contended".into()),
        mixrelay_render::ArrivalError::Invalid(reason) => {
            (false, format!("arrival invalid: {reason}"))
        }
    }
}
fn endpoint_observations(
    plan: &RenderPlan,
    indexes: &[usize],
    current: &BTreeMap<String, Observation>,
    contributors: &[Vec<usize>],
) -> Vec<Observation> {
    let mut selected = Vec::new();
    for index in indexes {
        let sink = &plan.sink_bindings()[*index];
        selected.extend(binding(current, sink.node_id().as_str(), false).copied());
        for source in &contributors[*index] {
            selected.extend(
                binding(
                    current,
                    plan.source_bindings()[*source].node_id().as_str(),
                    true,
                )
                .copied(),
            );
        }
    }
    selected.sort_by_key(|value| value.identity);
    selected.dedup_by_key(|value| value.identity);
    selected
}

// Missing snapshots skip a window; only independently retained metadata changes context.
// The second value names why the window is disturbed, for diagnostics.
fn endpoint_window(
    plan: &RenderPlan,
    indexes: &[usize],
    current: &BTreeMap<String, Observation>,
    previous: &BTreeMap<String, Observation>,
    contributors: &[Vec<usize>],
) -> (Vec<ClockContext>, Option<String>) {
    let mut contexts = previous.clone();
    contexts.extend(current.clone());
    let selected = endpoint_observations(plan, indexes, &contexts, contributors);
    let unsampled = |id: &str, input: bool| match binding(current, id, input) {
        None => Some(format!("{id}: no observation for the active plan")),
        Some(value) if !value.sampled => Some(format!("{id}: taps not sampled")),
        Some(_) => None,
    };
    let missing = indexes.iter().find_map(|index| {
        unsampled(plan.sink_bindings()[*index].node_id().as_str(), false).or_else(|| {
            contributors[*index].iter().find_map(|source| {
                unsampled(plan.source_bindings()[*source].node_id().as_str(), true)
            })
        })
    });
    let clocks = selected
        .iter()
        .map(|value| value.context())
        .collect::<Vec<_>>();
    let disturbed = missing.or_else(|| {
        selected.iter().find_map(|value| {
            let Some(old) = previous.values().find(|old| old.identity == value.identity) else {
                return Some(format!("device {}: first window", value.identity));
            };
            if value.context() != old.context() {
                return Some(format!("device {}: clock context changed", value.identity));
            }
            value
                .instability(*old)
                .map(|reason| format!("device {}: {reason}", value.identity))
        })
    });
    (clocks, disturbed)
}

impl Measurements {
    pub(super) fn observe(
        &mut self,
        plan: &RenderPlan,
        revision: u64,
        current: BTreeMap<String, Observation>,
        rates: &BTreeMap<mixrelay_audio_io::VirtualEndpointId, u32>,
    ) -> BTreeMap<mixrelay_audio_io::VirtualEndpointId, Option<u32>> {
        if self.revision != Some(revision) {
            self.endpoints.clear();
            self.previous.clear();
            self.revision = Some(revision);
        }
        #[allow(clippy::cast_precision_loss)]
        let ns_per_tick =
            mixrelay_realtime::Timebase::current().nanoseconds(1_000_000_000) as f64 / 1e9;
        let contributors = plan.destination_source_indexes();
        let delay = |id: &str, input| {
            let value = *binding(&current, id, input)?;
            let old = *binding(&self.previous, id, input)?;
            (value.sampled
                && old.sampled
                && value.context() == old.context()
                && value.graph_rate == plan.format().sample_rate)
                .then(|| value.delay_ns(old, input, ns_per_tick))
                .flatten()
        };
        let sources = plan
            .source_bindings()
            .iter()
            .map(|source| delay(source.node_id().as_str(), true))
            .collect::<Vec<_>>();
        let mut result = BTreeMap::new();
        for (id, rate) in rates {
            let indexes = plan
                .sink_bindings()
                .iter()
                .enumerate()
                .filter(|(_, sink)| sink.virtual_endpoint_id() == Some(id))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if indexes.is_empty() {
                continue;
            }
            let (clocks, mut diagnosis) =
                endpoint_window(plan, &indexes, &current, &self.previous, &contributors);
            let mut unstable = diagnosis.is_some();
            let arrivals = destination_arrivals(plan, &sources, *rate)
                .map_err(|(contended, reason)| {
                    unstable |= contended;
                    diagnosis.get_or_insert(reason);
                })
                .ok();
            let sinks = Sinks {
                plan,
                current: &current,
                delay: &delay,
                indexes: &indexes,
                rate: *rate,
            };
            let measured = sinks.measure(arrivals.as_deref());
            if measured.is_none() {
                diagnosis.get_or_insert_with(|| sinks.unmeasured(arrivals.as_deref()));
            }
            let endpoint = self.endpoints.entry(id.clone()).or_default();
            let value = endpoint.apply(*rate, &clocks, measured, unstable);
            let _ = endpoint.log(id, value, diagnosis);
            result.insert(id.clone(), value);
        }
        for (id, value) in current {
            if value.sampled
                || self
                    .previous
                    .get(&id)
                    .is_none_or(|old| old.context() != value.context())
            {
                self.previous.insert(id, value);
            }
        }
        result
    }
}
