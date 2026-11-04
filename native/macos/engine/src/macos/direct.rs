//! Callback-driven hardware rendering. All maps/processor storage are prepared
//! before publication. The supervisor only handles reconfiguration and lifetime;
//! it never schedules PCM or acknowledges pause while a render remains in flight.
mod buffers;
mod callback_io;
mod ownership;
mod renderer;
mod supervision;

#[cfg(test)]
mod continuity_tests;
#[cfg(test)]
mod measurement;
#[cfg(test)]
mod mixed_tests;
#[cfg(test)]
mod tests;

use super::AudioContext;
use crate::macos_devices::direct::{Binding, Descriptor, DirectPlan};
use mixrelay_render::BlockProcessor;
use ownership::PreparedRenderer;
use renderer::{GraphActivation, RenderSignals, Renderer};
use std::{sync::Arc, time::Instant};

/// Renders `processor` from `plan`'s master callback until the lifetime ends.
pub(super) fn run(
    plan: &DirectPlan,
    processor: &mut Box<BlockProcessor>,
    context: AudioContext<'_>,
) -> Result<(), String> {
    let signals = RenderSignals::default();
    let installed_plan = Arc::clone(processor.plan());
    let format = processor.plan().format();
    let renderer = Renderer {
        activation: Some(GraphActivation {
            installed: &installed_plan,
            context,
            rebind: &signals.rebind,
        }),
        ..Renderer::for_plan(plan, processor, context.runtime_status, &signals)
    };
    supervision::log_route(plan, format.sample_rate, format.block_frames);
    let mut prepared = PreparedRenderer::new(renderer);
    let installed = prepared.install(plan);
    let mut last_execution_report = Instant::now();
    signals.supervise(plan, context, &mut last_execution_report);
    drop(installed);
    plan.report_execution(last_execution_report.elapsed());
    signals.finish(plan, context)
}

/// Runs `drive` while `plan`'s master callback renders `processor`, installed as `run`
/// installs it, without `CoreAudio` or supervision.
#[cfg(test)]
pub(crate) fn render_directly<R>(
    plan: &DirectPlan,
    processor: &mut Box<BlockProcessor>,
    drive: impl FnOnce() -> R,
) -> R {
    let status = mixrelay_engine_core::session::AdapterRuntimeStatus::new(
        super::adapter_profile(),
        Arc::new(mixrelay_time::ManualClock::default()),
    );
    let signals = RenderSignals::default();
    let mut prepared =
        PreparedRenderer::new(Renderer::for_plan(plan, processor, &status, &signals));
    let installed = prepared.install(plan);
    let result = drive();
    drop(installed);
    assert!(
        !signals.failed.load(std::sync::atomic::Ordering::Acquire)
            && !signals.invalid.load(std::sync::atomic::Ordering::Acquire),
        "direct callback rejected its buffers"
    );
    result
}
