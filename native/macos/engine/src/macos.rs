//! Native processing-domain construction and graph-worker admission.
mod direct;
mod external_io;
mod format;
pub(crate) mod runtime_latency;
mod worker;
mod worker_runtime;
use external_io::{application_epoch, binding_sources, binding_token, read_application};
use mixrelay_audio_engine::{BlockProcessor, PeakMeterBank, RenderPlan};
use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler, RenderFormat};
use mixrelay_engine::session::AdapterRuntimeStatus;
use mixrelay_engine::worker_completion;
use std::sync::Arc;
pub use worker::{Error as AudioWorkerError, start as start_audio_worker};

#[cfg(test)]
pub(crate) use runtime_latency::tests::feedback_processor as test_feedback_processor;
#[cfg(test)]
mod input_alignment_tests;
use crate::endpoint_service;
#[cfg(test)]
use crate::macos_pacing::{ApplicationClockSnapshot, ApplicationPacer, Pace};
#[cfg(test)]
use external_io::{MacEngineExternalIo, source_token};
#[cfg(test)]
use mixrelay_audio_engine::{ExternalBinding, ExternalIo};
#[cfg(test)]
use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};
#[cfg(test)]
use std::time::Duration;
use worker::AudioContext;
#[cfg(test)]
use worker::{
    MacTimeline, NativePacers, RouteClock, WorkerCycle, next_native_pace, process_native_cycle,
};

#[cfg(any(test, feature = "test-util"))]
pub(crate) fn adapter_profile() -> mixrelay_engine::session::AdapterProfile {
    format::profile(mixrelay_audio_io::formats::SAMPLE_RATES.to_vec())
}

/// Creates native processing status before admitting any graph resources.
pub fn prepare_runtime(
    time: Arc<dyn mixrelay_engine::session::ProcessingTime>,
) -> AdapterRuntimeStatus {
    AdapterRuntimeStatus::new(
        format::profile(mixrelay_audio_io::formats::SAMPLE_RATES.to_vec()),
        time,
    )
    .with_graph_format_control()
}

/// Compiles the already selected startup document without publishing native resources.
/// # Errors
/// Returns graph compilation or render preparation failures.
pub fn create_initial_processor(
    document: &mixrelay_audio_graph::GraphDocument,
    format: RenderFormat,
    meters: Arc<PeakMeterBank>,
) -> Result<Box<BlockProcessor>, String> {
    let execution = GraphCompiler::default()
        .compile(
            document,
            CompileContext {
                format,
                limits: CompileLimits::default(),
            },
        )
        .map_err(|error| error.to_string())?;
    let render_plan = RenderPlan::prepare(&execution).map_err(|error| error.to_string())?;
    BlockProcessor::with_peak_meters(Arc::new(render_plan), meters)
        .map(Box::new)
        .map_err(|error| error.to_string())
}
