//! Selects one native pacing owner and observes route clock readiness.
use super::super::external_io::{binding_sources, binding_token};
use crate::endpoint_service;
use crate::macos_pacing::{ApplicationClockSnapshot, ApplicationPacer, HardwarePacer, Pace};
use mixrelay_macos_transport::{application_clock::ConsumerRoute, source_registry::SourceRegistry};
use mixrelay_render::{BlockProcessor, ExternalBinding, RenderPlan};

pub(in crate::macos) struct RouteClock {
    pub(in crate::macos) maximum_frames: u32,
    pub(in crate::macos) synchronized: bool,
    pub(in crate::macos) independent_idle: bool,
}
impl RouteClock {
    pub(in crate::macos) fn observe(
        processor: &BlockProcessor,
        devices: &crate::macos_devices::DeviceSet,
    ) -> Self {
        let mut result = Self {
            maximum_frames: processor.plan().format().block_frames,
            synchronized: true,
            independent_idle: true,
        };
        for (input, binding) in [
            (true, processor.plan().source_bindings()),
            (false, processor.plan().sink_bindings()),
        ]
        .into_iter()
        .flat_map(|(input, bindings)| bindings.iter().map(move |binding| (input, binding)))
        {
            let id = binding.node_id().as_str();
            if devices.is_binding_inactive(id, input) {
                continue;
            }
            if let Some(ready) = devices.clock_synchronized(id, input) {
                result.synchronized &= ready;
                result.independent_idle = false;
            } else if let Some(clock) = binding_clock(binding) {
                result.synchronized &= clock.synchronized != 0
                    && clock.graph_rate == processor.plan().format().sample_rate;
                result.independent_idle &= clock.running == 0;
            } else {
                result.synchronized = false;
                result.independent_idle = false;
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceKind {
    Application,
    Virtual,
}

#[derive(Default)]
pub(in crate::macos) struct NativePacers {
    hardware: HardwarePacer,
    application: ApplicationPacer,
    selected_application: Option<(u64, SourceKind)>,
}
impl NativePacers {
    /// The shared source this pacing consumes from its first credit, if any. The graph
    /// publishes it as the head route, so only that source's producer delivers PCM it
    /// retained before admission.
    pub(in crate::macos) fn head(&self) -> Option<u64> {
        // A device clock resets the application pacer, which then has no head.
        self.application.head()
    }

    fn select_application_master(&mut self, token: Option<(u64, SourceKind)>) {
        if token.is_none() {
            self.application = ApplicationPacer::default();
        }
        if token != self.selected_application
            && let Some(previous) = self.selected_application
        {
            set_source_master(previous, false);
        }
        self.selected_application = token;
        if let Some(token) = token {
            set_source_master(token, true);
        }
    }
}
impl Drop for NativePacers {
    fn drop(&mut self) {
        self.select_application_master(None);
    }
}

/// Virtual sources of the active plan. Graph cycles publish each one's route; a source
/// the next plan drops, or every source when the worker ends, is published unrouted.
/// The set lives for the worker, so checkpoints and gated rendering keep every route.
pub(in crate::macos) struct PlanRoutes {
    sources: Option<&'static SourceRegistry>,
    tokens: Vec<u64>,
}
impl PlanRoutes {
    pub(in crate::macos) fn new(sources: Option<&'static SourceRegistry>) -> Self {
        Self {
            sources,
            tokens: Vec::new(),
        }
    }

    /// Adopts a newly activated plan. Allocates; call outside realtime.
    pub(in crate::macos) fn activate(&mut self, plan: &RenderPlan) {
        let tokens: Vec<_> = plan
            .source_bindings()
            .iter()
            .filter(|binding| binding.virtual_endpoint_id().is_some())
            .map(binding_token)
            .collect();
        for token in self.tokens.iter().filter(|token| !tokens.contains(token)) {
            self.unroute(*token);
        }
        self.tokens = tokens;
    }
    fn unroute(&self, token: u64) {
        if let Some(sources) = self.sources {
            sources.set_consumer_route(token, ConsumerRoute::Unrouted);
        }
    }
}
impl Drop for PlanRoutes {
    fn drop(&mut self) {
        for token in &self.tokens {
            self.unroute(*token);
        }
    }
}

fn set_source_master((token, kind): (u64, SourceKind), master: bool) {
    let sources = match kind {
        SourceKind::Virtual => endpoint_service::virtual_source_registry(),
        SourceKind::Application => endpoint_service::source_registry(),
    };
    if let Some(sources) = sources {
        sources.set_clock_master(token, master);
    }
}

fn binding_clock(binding: &ExternalBinding) -> Option<ApplicationClockSnapshot> {
    let progress = binding_sources(binding, endpoint_service::source_registry())?
        .pcm_progress(binding_token(binding))?;
    let clock = progress.clock;
    Some(ApplicationClockSnapshot {
        epoch: clock.epoch,
        published_frames: progress.published_frames,
        native_rate: clock.native_rate,
        graph_rate: clock.graph_rate,
        running: clock.running,
        synchronized: clock.synchronized,
        capacity_frames: progress.capacity_frames,
    })
}

fn application_identity(binding: &ExternalBinding) -> Option<(u64, SourceKind)> {
    let kind = if binding.virtual_endpoint_id().is_some() {
        SourceKind::Virtual
    } else if binding.kind() == mixrelay_audio_graph::NodeKind::ApplicationCapture {
        SourceKind::Application
    } else {
        return None;
    };
    Some((binding_token(binding), kind))
}

/// A shared source may pace the graph only while its node is on and the plan carries it
/// to an output: an Off or metered-only source must never hold the graph on its credit.
fn paces(plan: &RenderPlan, binding: &ExternalBinding) -> Option<(u64, SourceKind)> {
    let enabled = plan
        .controls()
        .snapshot(binding.node_id())
        .is_some_and(|(control, _)| control.enabled);
    application_identity(binding).filter(|_| enabled && binding.is_routed_source())
}

pub(in crate::macos) fn next_native_pace(
    pacers: &mut NativePacers,
    devices: &crate::macos_devices::DeviceSet,
    plan: &RenderPlan,
    capacity: u32,
) -> (Pace, u64) {
    // Device-owned frame credit remains authoritative across Driver and physical IOProcs.
    if let Some(hardware) = devices.hardware_clock_progress() {
        let format = plan.format();
        devices.set_clock_master(Some(hardware.owner_id));
        devices.set_graph_quantum(
            u32::try_from(
                (u64::from(hardware.quantum) * u64::from(format.sample_rate))
                    .div_ceil(u64::from(hardware.native_rate)),
            )
            .unwrap_or(u32::MAX)
            .max(format.block_frames),
        );
        pacers.select_application_master(None);
        return (
            pacers.hardware.next(
                (hardware.epoch, hardware.native_frames, hardware.native_rate),
                format.sample_rate,
                format.block_frames,
                capacity,
            ),
            2,
        );
    }
    devices.set_clock_master(None);
    devices.set_graph_quantum(plan.format().block_frames);
    next_application_pace(pacers, plan, binding_clock)
}

/// Paces the graph from its current master while it runs and may still pace, else from
/// the first running pacing source in plan order, or awaits the first one when none
/// runs. Keeping a running master avoids a new pacer generation and a reacquired capture
/// cursor when an earlier-listed source starts or comes back on. `clock` observes a
/// binding's producer.
fn next_application_pace(
    pacers: &mut NativePacers,
    plan: &RenderPlan,
    clock: impl Fn(&ExternalBinding) -> Option<ApplicationClockSnapshot>,
) -> (Pace, u64) {
    let format = plan.format();
    let running = |binding: &ExternalBinding| {
        let identity = paces(plan, binding)?;
        clock(binding)
            .filter(|clock| clock.running != 0)
            .map(|clock| (identity, clock))
    };
    let current = pacers.selected_application.and_then(|selected| {
        plan.source_bindings()
            .iter()
            .find(|binding| paces(plan, binding) == Some(selected))
            .and_then(running)
    });
    if let Some((identity, clock)) =
        current.or_else(|| plan.source_bindings().iter().find_map(running))
    {
        pacers.select_application_master(Some(identity));
        return (
            pacers.application.next(
                (identity.0, clock),
                (
                    format.sample_rate,
                    format.block_frames,
                    clock.capacity_frames,
                ),
            ),
            3,
        );
    }
    // Master selection follows graph identity. A producer format rearm can
    // temporarily hide native running state without removing the selected source.
    let retained = pacers.selected_application.filter(|selected| {
        plan.source_bindings()
            .iter()
            .any(|binding| paces(plan, binding) == Some(*selected))
    });
    let waiting = retained.or_else(|| {
        plan.source_bindings()
            .iter()
            .find_map(|binding| paces(plan, binding))
    });
    if let Some((token, _)) = waiting {
        pacers.application.await_source(token);
    }
    pacers.select_application_master(waiting);
    (Pace::Unknown, 0)
}

#[cfg(test)]
mod tests;
