//! One-second control publication of measured content residence, fenced by active topology.
mod application;
mod arithmetic;
#[cfg(test)]
pub(crate) mod fixture;
mod measurements;
#[cfg(test)]
mod route_tests;
mod stable_endpoint;
#[cfg(test)]
mod tests;

use crate::macos_devices::MacDevices;
use application::append_observations;
pub(super) use application::record_sources;
use measurements::Measurements;
use mixrelay_driver_core::{EngineLatencyLease, LatencyCommand, RuntimeLatency};
use mixrelay_render::{ProcessorExchange, RenderPlan};
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
