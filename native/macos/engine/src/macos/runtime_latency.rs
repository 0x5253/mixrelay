//! One-second control publication of measured content residence, fenced by active topology.
mod application;
use crate::macos_devices::{
    MacDevices,
    latency::{Observation, binding},
};
use application::append_observations;
pub(super) use application::record_sources;
use mixrelay_driver_core::{EngineLatencyLease, LatencyCommand, RuntimeLatency};
use mixrelay_render::{ProcessorExchange, RenderPlan, content_latency::LatencyHysteresis};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::{sync::oneshot, task::JoinHandle};

/// Driver publication consumed by the native latency observer.
pub trait LatencyPublication: Send + Sync {
    /// Reads the current Driver endpoints without changing their native format.
    fn snapshot(&self) -> PublicationFuture<'_, mixrelay_driver_core::DriverSnapshot>;
    /// Claims, updates or releases this engine's measured latency lease.
    fn publish_latency<'a>(
        &'a self,
        request: &'a LatencyCommand,
    ) -> PublicationFuture<'a, mixrelay_driver_core::LatencyReply>;
}

/// Original transport failures returned by the application composition adapter.
pub type PublicationError = Box<dyn std::error::Error + Send + Sync>;
/// One bounded control-plane operation, never polled by an audio callback.
pub type PublicationFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, PublicationError>> + Send + 'a>>;

pub struct Reporter {
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}
impl Reporter {
    pub fn start(
        exchange: Arc<ProcessorExchange>,
        devices: Arc<MacDevices>,
        incarnation: String,
        client: Arc<dyn LatencyPublication>,
    ) -> Self {
        let (stop, receive) = oneshot::channel();
        let worker = tokio::spawn(observe(client, exchange, devices, incarnation, receive));
        Self {
            stop: Some(stop),
            worker: Some(worker),
        }
    }
    /// Stops publication and joins its observer task.
    /// # Errors
    /// Returns an observer panic or cancellation failure.
    pub async fn finish(mut self) -> Result<(), String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.await.map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
impl Drop for Reporter {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // The task releases its lease when signalled. Abrupt process death uses service expiry.
    }
}
async fn observe(
    client: Arc<dyn LatencyPublication>,
    exchange: Arc<ProcessorExchange>,
    devices: Arc<MacDevices>,
    incarnation: String,
    mut stop: oneshot::Receiver<()>,
) {
    let mut lease: Option<EngineLatencyLease> = None;
    let mut previous_error = None;
    let mut measurements = Measurements::default();
    loop {
        let result = publish(
            client.as_ref(),
            (&exchange, &devices),
            &incarnation,
            &mut lease,
            &mut measurements,
        )
        .await;
        match result {
            Ok(()) => previous_error = None,
            Err(error) => {
                if previous_error.as_ref() != Some(&error) {
                    tracing::warn!(%error, "input latency publication unavailable");
                }
                previous_error = Some(error);
            }
        }
        tokio::select! { _ = &mut stop => break, () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {} }
    }
    if let Some(lease) = lease {
        let _ = client
            .publish_latency(&LatencyCommand::Release { lease })
            .await;
    }
}
/// Claims a fresh lease with fresh measurements.
async fn claim(
    client: &dyn LatencyPublication,
    incarnation: &str,
    lease: &mut Option<EngineLatencyLease>,
    measurements: &mut Measurements,
) -> Result<(), String> {
    *measurements = Measurements::default();
    let reply = client
        .publish_latency(&LatencyCommand::Claim {
            incarnation: incarnation.into(),
            process_id: std::process::id(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if !reply.accepted {
        return Err("previous engine latency lease has not expired".into());
    }
    *lease = reply.lease;
    Ok(())
}
async fn publish(
    client: &dyn LatencyPublication,
    runtime: (&ProcessorExchange, &MacDevices),
    incarnation: &str,
    lease: &mut Option<EngineLatencyLease>,
    measurements: &mut Measurements,
) -> Result<(), String> {
    let (exchange, devices) = runtime;
    if lease.is_none() {
        claim(client, incarnation, lease, measurements).await?;
    }
    let owner = lease
        .as_ref()
        .ok_or("Driver omitted the engine latency lease")?;
    let mut destinations = std::collections::BTreeMap::new();
    let mut validity = None;
    if let Some(plan) = exchange.latency_plan() {
        let settings = client.snapshot().await.map_err(|error| error.to_string())?;
        let revision = exchange.revisions().active;
        let mut observations =
            devices.latency_observations(plan.revision(), exchange.has_pending_latency());
        append_observations(
            &plan,
            super::endpoint_service::source_registry(),
            exchange.has_pending_latency(),
            &mut observations,
        );
        let rates = settings
            .settings
            .devices
            .iter()
            .filter_map(|device| {
                device
                    .observed_sample_rate
                    .map(|rate| (device.spec.id.clone(), rate))
            })
            .collect::<BTreeMap<_, _>>();
        let values = measurements.observe(&plan, plan.revision(), observations, &rates);
        // Publication races must not label a new graph with an older measurement.
        let valid = publication_valid(exchange, &plan, revision);
        validity = Some((plan.revision(), revision, valid));
        for (endpoint_id, rate) in rates {
            if !plan
                .sink_bindings()
                .iter()
                .any(|sink| sink.virtual_endpoint_id() == Some(&endpoint_id))
            {
                continue;
            }
            destinations.insert(
                endpoint_id.clone(),
                RuntimeLatency {
                    endpoint_id: endpoint_id.clone(),
                    rate,
                    engine_lease: owner.clone(),
                    applied_graph_revision: revision,
                    input_latency_frames: valid
                        .then(|| values.get(&endpoint_id).copied().flatten())
                        .flatten(),
                },
            );
        }
    }
    measurements.log_publication(publication_summary(
        validity,
        exchange.plan_revisions().active,
        &destinations,
    ));
    // An empty set still carries the applied revision so it fences the endpoints it drops.
    let revision = validity.map_or_else(|| exchange.revisions().active, |(_, applied, _)| applied);
    let reply = client
        .publish_latency(&LatencyCommand::Publish {
            lease: owner.clone(),
            revision,
            destinations: destinations.into_values().collect(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if !reply.accepted {
        *lease = None;
    }
    Ok(())
}

/// `validity` is the latency plan's revision, the applied graph revision and whether they
/// still match the exchange; `None` when no latency plan matches the active plan revision.
fn publication_summary(
    validity: Option<(u64, u64, bool)>,
    active_plan: u64,
    destinations: &BTreeMap<mixrelay_audio_io::VirtualEndpointId, RuntimeLatency>,
) -> String {
    let Some((plan, applied, valid)) = validity else {
        return format!("no latency plan for active plan revision {active_plan}");
    };
    let endpoints = destinations
        .values()
        .map(|entry| match entry.input_latency_frames {
            Some(frames) => format!("{}={frames}", entry.endpoint_id.as_str()),
            None => format!("{}=unknown", entry.endpoint_id.as_str()),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("plan {plan}, applied graph {applied}, valid {valid}: [{endpoints}]")
}

fn publication_valid(
    exchange: &ProcessorExchange,
    plan: &RenderPlan,
    applied_revision: u64,
) -> bool {
    exchange.revisions().active == applied_revision
        && exchange.plan_revisions().active == plan.revision()
}

// These maps and arithmetic live on control. Callback taps own no endpoint maps.
#[derive(Default)]
struct Measurements {
    revision: Option<u64>,
    previous: BTreeMap<String, Observation>,
    endpoints: BTreeMap<mixrelay_audio_io::VirtualEndpointId, StableEndpoint>,
    /// The last logged publication summary, so each change logs once.
    publication: Option<String>,
}
impl Measurements {
    /// Logs what this tick sends to the Driver when it differs from the previous tick: whether a
    /// latency plan exists, whether the publication is valid, and each endpoint's frames.
    fn log_publication(&mut self, summary: String) -> bool {
        if self.publication.as_ref() == Some(&summary) {
            return false;
        }
        tracing::info!(%summary, "input latency publication");
        self.publication = Some(summary);
        true
    }
}
type ClockContext = (u64, u64, u64, u64, u32, u32, bool, u64);
#[derive(Default)]
struct StableEndpoint {
    rate: u32,
    clocks: Vec<ClockContext>,
    hysteresis: LatencyHysteresis,
    value: Option<u32>,
    /// The last logged reason a window could not be measured, so each change logs once.
    diagnosis: Option<String>,
}
impl StableEndpoint {
    fn apply(
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
    fn log(
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
fn threshold(rate: u32) -> u32 {
    32.max(rate.div_ceil(1_000))
}
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn frames(ns: f64, rate: u32) -> Option<u32> {
    let value = (ns * f64::from(rate) / 1e9).ceil();
    (rate != 0 && ns >= 0.0 && value.is_finite() && value <= f64::from(u32::MAX))
        .then_some(value as u32)
}
fn agree(values: impl IntoIterator<Item = Option<u32>>, threshold: u32) -> Option<u32> {
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
    fn observe(
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

#[cfg(test)]
mod route_tests;
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn feedback_plan() -> RenderPlan {
        RenderPlan::prepare(&feedback_execution()).unwrap()
    }
    pub(crate) fn feedback_processor(
        factory: &dyn mixrelay_render::HostedProcessorFactory,
        meters: Arc<mixrelay_render::PeakMeterBank>,
    ) -> mixrelay_render::BlockProcessor {
        mixrelay_render::BlockProcessor::prepare_with_hosted_processors(
            &feedback_execution(),
            meters,
            factory,
        )
        .unwrap()
    }
    fn feedback_execution() -> mixrelay_audio_graph::ExecutionPlan {
        use mixrelay_audio_graph::{
            CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
        };
        let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"revision":1,"nodes":[{
                "id":"virtual","type":"virtual_endpoint",
                "config":{"id":"01010101-0101-0101-0101-010101010101","displayName":"Fixture",
                    "reference":{"render":true,"capture":true},
                    "render":{"channels":2,"channelLayout":"stereo"},"capture":{"channels":2,"channelLayout":"stereo"},
                    "supportedSampleRates":[48000],"defaultSampleRate":48000}
            }, {"id":"gain","type":"gain","config":{"channels":2,"gainDb":0}}],"edges":[
                {"id":"input","source":{"nodeId":"virtual","portId":"from_os"},"destination":{"nodeId":"gain","portId":"audio"}},
                {"id":"feedback","source":{"nodeId":"gain","portId":"audio"},"destination":{"nodeId":"virtual","portId":"to_os"}}]
        })).unwrap();
        if let mixrelay_audio_graph::NodeConfig::VirtualEndpoint(endpoint) =
            &document.nodes[0].config
        {
            document.nodes[0].config =
                mixrelay_audio_graph::NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
        }
        GraphCompiler::default()
            .compile(
                &document,
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 32,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap()
    }
    fn observation() -> Observation {
        use mixrelay_audio_io::residence::ResidenceSnapshot;
        use mixrelay_macos_transport::device_io::LatencySnapshot;
        Observation {
            identity: 1,
            epoch: 2,
            graph_epoch: 5,
            generation: 3,
            clock_breaks: 0,
            native_rate: 48_000,
            graph_rate: 48_000,
            direct: true,
            ready: true,
            sampled: true,
            faults: 0,
            native: LatencySnapshot::default(),
            src_input: ResidenceSnapshot::default(),
            src_output: ResidenceSnapshot::default(),
            src_delay: 0,
        }
    }

    fn canonical_snapshot_route() -> mixrelay_audio_graph::composition::ComposedGraph {
        use mixrelay_audio_graph::composition::{
            DeviceReadReference, DeviceReference, DeviceRegistry, GraphFragment, compose,
        };
        use mixrelay_audio_graph::{
            CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, RenderFormat,
        };
        let fixture = feedback_execution();
        let mut devices = Vec::new();
        for id in ["A", "D"] {
            let NodeConfig::VirtualEndpointTransport(spec) = fixture
                .nodes()
                .iter()
                .find_map(|node| {
                    matches!(node.config(), NodeConfig::VirtualEndpointTransport(_))
                        .then_some(node.config())
                })
                .unwrap()
            else {
                unreachable!()
            };
            let mut spec = spec.clone();
            spec.id = id.into();
            devices.push(spec);
        }
        let registry = DeviceRegistry::new(devices).unwrap();
        let document: GraphDocument = serde_json::from_value(serde_json::json!({
            "schemaVersion":2,"revision":1,"nodes":[
                {"id":"gain","type":"gain","config":{"channels":2,"gainDb":0}}
            ],"edges":[
                {"id":"in","source":{"nodeId":"a","portId":"from_os"},"destination":{"nodeId":"gain","portId":"audio"}},
                {"id":"out","source":{"nodeId":"gain","portId":"audio"},"destination":{"nodeId":"d","portId":"to_os"}}
            ]
        })).unwrap();
        let references = [
            DeviceReference {
                node_id: "a".into(),
                device_id: "A".into(),
                control: mixrelay_audio_graph::NodeControl::default(),
                read: Some(DeviceReadReference { channels: 2 }),
                write_channels: None,
                gain_db: 0.0,
            },
            DeviceReference {
                node_id: "d".into(),
                device_id: "D".into(),
                control: mixrelay_audio_graph::NodeControl::default(),
                read: None,
                write_channels: Some(2),
                gain_db: 0.0,
            },
        ];
        compose(
            &registry,
            &[GraphFragment {
                id: "route",
                document: &document,
                device_references: &references,
                source_references: &[],
                output_references: &[],
            }],
            1,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
            &GraphCompiler::default(),
        )
        .unwrap()
    }

    #[test]
    fn canonical_snapshot_route_publishes_measured_source_plus_prepared_quantum() {
        use mixrelay_audio_io::residence::ResidenceSnapshot;
        use mixrelay_render::BlockProcessor;
        let composed = canonical_snapshot_route();
        let mut processor = BlockProcessor::prepare_with_hosted_processors(
            &composed.execution,
            Arc::new(mixrelay_render::PeakMeterBank::new(32, 64)),
            &crate::macos_devices::latency::tests::Factory,
        )
        .unwrap();
        let plan = Arc::clone(processor.plan());
        assert_eq!(plan.source_bindings().len(), 1);
        assert_eq!(
            plan.source_bindings()[0].virtual_endpoint_id(),
            Some(&"A".into())
        );
        assert_eq!(plan.sink_bindings().len(), 1);
        assert_eq!(
            plan.sink_bindings()[0].virtual_endpoint_id(),
            Some(&"D".into())
        );
        let mut d_pcm = [0.0; 64];
        for expected in [0.0, 0.25] {
            processor
                .process_block(32, &[&[0.25; 64]], &mut [&mut d_pcm])
                .unwrap();
            assert_eq!(d_pcm.map(f32::to_bits), [expected; 64].map(f32::to_bits));
        }
        assert!(processor.acknowledge_content_latency());
        let a = composed.native_devices[&"A".into()].as_str();
        let d = composed.native_devices[&"D".into()].as_str();
        let rates = BTreeMap::from([("D".into(), 48_000)]);
        let mut source = observation();
        let mut destination = observation();
        destination.identity += 1;
        let current =
            |source, destination| BTreeMap::from([(a.into(), source), (d.into(), destination)]);
        let mut measurement = Measurements::default();
        assert_eq!(
            measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
            None
        );
        let ticks =
            mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_millis(1));
        source.native.acquisition = ResidenceSnapshot {
            sum: ticks,
            weight: 1,
            invalid: 0,
        };
        destination.native.presentation = source.native.acquisition;
        let frames = measurement.observe(&plan, 1, current(source, destination), &rates)
            [&"D".into()]
            .unwrap();
        assert!(
            (128..=129).contains(&frames),
            "2 ms plus 32 graph frames: {frames}"
        );
        // Source disturbances must still reach the destination through its bus.
        source.faults += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
            Some(frames)
        );
        source.epoch += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(source, destination), &rates)[&"D".into()],
            None
        );
    }

    #[test]
    fn control_edits_keep_topology_measurements_and_publish_applied_control_revision() {
        use mixrelay_render::{BlockProcessor, RuntimeControlUpdate};
        let plan = Arc::new(feedback_plan());
        let exchange = ProcessorExchange::new(plan.revision());
        let mut processor = Box::new(BlockProcessor::new(Arc::clone(&plan)).unwrap());
        exchange.observe_plan(Arc::clone(&plan));
        let mut measurements = Measurements::default();
        let id = plan.sink_bindings()[0]
            .virtual_endpoint_id()
            .unwrap()
            .clone();
        let rates = BTreeMap::from([(id.clone(), 48_000)]);
        let mut value = observation();
        let mut observe = |value| {
            measurements.observe(
                &plan,
                plan.revision(),
                BTreeMap::from([("virtual".into(), value)]),
                &rates,
            )[&id]
        };
        assert_eq!(observe(value), None);
        for (revision, muted) in [(2, true), (3, false)] {
            exchange
                .publish_controls(
                    plan.controls(),
                    revision,
                    &[RuntimeControlUpdate {
                        node_id: "gain".into(),
                        control: mixrelay_audio_graph::NodeControl {
                            muted,
                            ..Default::default()
                        },
                        gain_db: Some(-6.0 * f32::from(muted)),
                    }],
                )
                .unwrap();
            processor
                .process_block(32, &[&[0.0; 64]], &mut [&mut [0.0; 64]])
                .unwrap();
            assert_eq!(exchange.revisions().active, revision);
            assert_eq!(exchange.latency_plan().unwrap().revision(), 1);
            value.native.acquisition.weight += 1;
            value.native.presentation.weight += 1;
            assert_eq!(observe(value), Some(0));
            assert!(publication_valid(&exchange, &plan, revision));
            assert!(!publication_valid(&exchange, &plan, revision - 1));
        }
        let next = feedback_plan();
        // A mismatched exchange topology is sufficient to exercise the publication fence.
        let different = ProcessorExchange::new(4);
        assert!(!publication_valid(&different, &next, 4));
    }

    #[test]
    fn contended_or_missing_taps_skip_windows_but_real_clock_changes_reset() {
        let plan = feedback_plan();
        let id = plan.sink_bindings()[0]
            .virtual_endpoint_id()
            .unwrap()
            .clone();
        let rates = BTreeMap::from([(id.clone(), 48_000)]);
        let mut measurement = Measurements::default();
        let mut value = observation();
        let current = |value| BTreeMap::from([("virtual".into(), value)]);
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            None
        );
        value.native.acquisition.weight += 1;
        value.native.presentation.weight += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(0)
        );
        let mut busy = value;
        busy.sampled = false;
        assert_eq!(
            measurement.observe(&plan, 1, current(busy), &rates)[&id],
            Some(0)
        );
        assert_eq!(
            measurement.observe(&plan, 1, BTreeMap::new(), &rates)[&id],
            Some(0)
        );
        value.native.acquisition.weight += 1;
        value.native.presentation.weight += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(0)
        );
        for change in 0..4 {
            let mut changed = value;
            changed.sampled = false;
            match change {
                0 => changed.native_rate = 44_100,
                1 => changed.epoch += 1,
                2 => changed.generation += 1,
                _ => changed.identity += 1,
            }
            assert_eq!(
                measurement.observe(&plan, 1, current(changed), &rates)[&id],
                None
            );
        }
    }

    #[test]
    fn unusable_windows_name_their_first_reason_once() {
        let plan = feedback_plan();
        let id = plan.sink_bindings()[0]
            .virtual_endpoint_id()
            .unwrap()
            .clone();
        let rates = BTreeMap::from([(id.clone(), 48_000)]);
        let mut measurement = Measurements::default();
        let mut value = observation();
        let current = |value| BTreeMap::from([("virtual".into(), value)]);
        let reason = |measurement: &Measurements| measurement.endpoints[&id].diagnosis.clone();
        measurement.observe(&plan, 1, BTreeMap::new(), &rates);
        assert_eq!(
            reason(&measurement).as_deref(),
            Some("virtual: no observation for the active plan")
        );
        measurement.observe(&plan, 1, current(value), &rates);
        assert_eq!(
            reason(&measurement).as_deref(),
            Some("device 1: first window")
        );
        value.native.acquisition.weight += 1;
        value.native.presentation.weight += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(0)
        );
        assert_eq!(reason(&measurement), None);
        value.faults += 1;
        measurement.observe(&plan, 1, current(value), &rates);
        assert_eq!(
            reason(&measurement).as_deref(),
            Some("device 1: binding faults advanced")
        );
        value.native_rate = 44_100;
        value.native.acquisition.weight += 1;
        value.native.presentation.weight += 1;
        measurement.observe(&plan, 1, current(value), &rates);
        assert_eq!(
            reason(&measurement).as_deref(),
            Some("device 1: clock context changed")
        );
        // An unsampled tap is named before the fault it also carries.
        let mut both = value;
        both.sampled = false;
        both.faults += 1;
        measurement.observe(&plan, 1, current(both), &rates);
        assert_eq!(
            reason(&measurement).as_deref(),
            Some("virtual: taps not sampled")
        );
    }

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
    fn publication_summaries_log_each_change_once() {
        let lease = EngineLatencyLease {
            generation: 1,
            incarnation: "run".into(),
            process_id: 2,
        };
        let id = mixrelay_audio_io::VirtualEndpointId::new("device".into());
        let entry = |frames| RuntimeLatency {
            endpoint_id: id.clone(),
            rate: 48_000,
            engine_lease: lease.clone(),
            applied_graph_revision: 7,
            input_latency_frames: frames,
        };
        let destinations = |frames| BTreeMap::from([(id.clone(), entry(frames))]);
        assert_eq!(
            publication_summary(None, 4, &BTreeMap::new()),
            "no latency plan for active plan revision 4"
        );
        assert_eq!(
            publication_summary(Some((4, 7, true)), 4, &destinations(Some(384))),
            "plan 4, applied graph 7, valid true: [device=384]"
        );
        assert_eq!(
            publication_summary(Some((4, 7, false)), 4, &destinations(None)),
            "plan 4, applied graph 7, valid false: [device=unknown]"
        );
        let mut measurements = Measurements::default();
        let summary = || publication_summary(Some((4, 7, true)), 4, &destinations(Some(384)));
        assert!(measurements.log_publication(summary()));
        assert!(!measurements.log_publication(summary()));
        assert!(measurements.log_publication(publication_summary(None, 5, &BTreeMap::new())));
    }

    #[test]
    fn arrival_contention_disturbs_while_an_invalid_request_is_unknown() {
        assert_eq!(
            arrival_failure(mixrelay_render::ArrivalError::Contended),
            (true, "arrival read contended".into())
        );
        assert_eq!(
            arrival_failure(mixrelay_render::ArrivalError::Invalid("size".into())),
            (false, "arrival invalid: size".into())
        );
    }

    #[test]
    fn reporter_windows_publish_real_direct_delay_and_reset_graph_and_lease() {
        use mixrelay_audio_io::residence::ResidenceSnapshot;
        let plan = feedback_plan();
        let id = plan.sink_bindings()[0]
            .virtual_endpoint_id()
            .unwrap()
            .clone();
        let rates = BTreeMap::from([(id.clone(), 48_000)]);
        let mut value = observation();
        let current = |value| BTreeMap::from([("virtual".into(), value)]);
        let mut measurement = Measurements::default();
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            None
        );
        let ticks =
            mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_millis(1));
        value.native.acquisition = ResidenceSnapshot {
            sum: ticks,
            weight: 1,
            invalid: 0,
        };
        value.native.presentation = value.native.acquisition;
        let first = measurement.observe(&plan, 1, current(value), &rates)[&id].unwrap();
        assert!((96..=97).contains(&first));
        value.faults += 1;
        assert_eq!(
            measurement.observe(&plan, 1, current(value), &rates)[&id],
            Some(first)
        );
        assert_eq!(
            measurement.observe(&plan, 2, current(value), &rates)[&id],
            None
        );
        // A freshly claimed lease has fresh window baselines and no retained values.
        measurement = Measurements::default();
        assert_eq!(
            measurement.observe(&plan, 2, current(value), &rates)[&id],
            None
        );
        value.native.acquisition.invalid += 1;
        assert_eq!(
            measurement.observe(&plan, 2, current(value), &rates)[&id],
            None
        );
    }
    #[test]
    fn residence_invalid_windows_retain_published_latency_and_then_recover() {
        use mixrelay_audio_io::residence::{Passage, Residence};
        let plan = feedback_plan();
        let id = plan.sink_bindings()[0]
            .virtual_endpoint_id()
            .unwrap()
            .clone();
        let rates = BTreeMap::from([(id.clone(), 48_000)]);
        // Just under 32 native frames at 48 kHz, so the planned 32 round up to 64 once.
        let ticks =
            mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_micros(666));
        for index in 0..4 {
            let mut measurement = Measurements::default();
            let mut value = observation();
            value.direct = false;
            let current = |value| BTreeMap::from([("virtual".into(), value)]);
            assert_eq!(
                measurement.observe(&plan, 1, current(value), &rates)[&id],
                None
            );
            let advance = |value: &mut Observation| {
                for tap in [
                    &mut value.native.acquisition,
                    &mut value.native.presentation,
                    &mut value.src_input,
                    &mut value.src_output,
                ] {
                    tap.weight += 32;
                }
                for queue in [
                    &mut value.native.native_queue,
                    &mut value.native.graph_queue,
                ] {
                    queue.producer.position += 32;
                    queue.consumer.position += 32;
                }
                value.native.native_queue.consumer.area += 32 * ticks;
            };
            advance(&mut value);
            assert_eq!(
                measurement.observe(&plan, 1, current(value), &rates)[&id],
                Some(64)
            );
            // A FIFO consumer passes frames it never read, or an SRC sample is invalid.
            // No shortage/fault counter changes in this window.
            if index < 2 {
                let passage = Passage::default();
                passage.consumer.advance(32, 0, 0);
                let discarded = passage.consumer.snapshot().unwrap().discarded;
                let queue = if index == 0 {
                    &mut value.native.native_queue
                } else {
                    &mut value.native.graph_queue
                };
                queue.consumer.discarded += discarded;
            } else {
                let residence = Residence::default();
                residence.record(None, 32);
                let invalid = residence.snapshot().unwrap().invalid;
                let tap = if index == 2 {
                    &mut value.src_input
                } else {
                    &mut value.src_output
                };
                tap.invalid += invalid;
            }
            assert_eq!(
                measurement.observe(&plan, 1, current(value), &rates)[&id],
                Some(64)
            );
            advance(&mut value);
            assert_eq!(
                measurement.observe(&plan, 1, current(value), &rates)[&id],
                Some(64)
            );
        }
    }
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
