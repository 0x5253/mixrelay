//! Engine-wide `PipeWire` profile, startup helpers and the Driver port.
use crate::{backend::LinuxVirtualEndpointBackend, format};
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
};
use mixrelay_audio_io::session::{
    AudioBackendKind, AudioSessionCapabilities, CURRENT_TIMER_FORMAT, ClockAuthority,
};
use mixrelay_engine_core::session::{AdapterProfile, AdapterRuntimeStatus, ProcessingTime};
use mixrelay_engine_host::resource_reclaimer::ResourceReclaimer;
use mixrelay_render::{BlockProcessor, PeakMeterBank, ProcessorExchange, RenderPlan};
use std::sync::Arc;

pub(crate) const ENGINE_SAMPLE_RATE: u32 = CURRENT_TIMER_FORMAT.sample_rate;
const ENGINE_BLOCK_FRAMES: u32 = CURRENT_TIMER_FORMAT.block_frames;
pub(crate) const MAX_ENDPOINT_CHANNELS: u16 = 64;

/// Driver observations and revision-checked mutations consumed by native graph resources.
pub trait EndpointDriver: Send + Sync {
    /// # Errors
    /// Returns the original Driver transport or protocol failure.
    fn snapshot(
        &self,
    ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>;
    /// # Errors
    /// Returns stale revisions, unsupported formats or transport failure.
    fn mutate(
        &self,
        command: &mixrelay_driver_core::DriverCommand,
    ) -> Result<mixrelay_driver_core::DriverSnapshot, mixrelay_driver_core::DriverError>;
}

/// Describes the native `PipeWire` processing capabilities.
#[must_use]
pub fn adapter_profile() -> AdapterProfile {
    AdapterProfile {
        initial_format: CURRENT_TIMER_FORMAT,
        capabilities: AudioSessionCapabilities {
            backend: AudioBackendKind::PipeWire,
            supported_sample_rates: format::SAMPLE_RATES.to_vec(),
            supported_block_frames: format::BUFFER_SIZES.to_vec(),
            can_reconfigure_live: true,
            hardware_input: true,
            hardware_output: true,
            virtual_endpoint: true,
            application_capture: true,
            sample_rate_conversion: true,
            drift_correction: false,
        },
        application_capture_rates: format::SAMPLE_RATES.to_vec(),
        clock_authority: ClockAuthority::AudioServer,
        transport_latency_frames: None,
        requires_observed_device_rate: false,
    }
}

/// Creates status policy with this backend's independently owned endpoint observations.
#[must_use]
pub fn prepare_runtime(
    time: Arc<dyn ProcessingTime>,
    observation: Arc<format::EndpointObservation>,
) -> AdapterRuntimeStatus {
    AdapterRuntimeStatus::new(adapter_profile(), time)
        .with_graph_format_control()
        .with_observed_rate_following(move || observation.rate())
}

/// Compiles the selected startup graph without publishing native resources.
/// # Errors
/// Returns graph compilation or DSP preparation failure.
pub fn create_initial_processor(
    document: &GraphDocument,
    meters: Arc<PeakMeterBank>,
    format: RenderFormat,
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
    let render = RenderPlan::prepare(&execution).map_err(|error| error.to_string())?;
    BlockProcessor::with_peak_meters(Arc::new(render), meters)
        .map(Box::new)
        .map_err(|error| error.to_string())
}

/// Starts control-side reclamation for native resources and retired DSP plans.
/// # Errors
/// Returns thread creation failure without leaving a reclaimer detached.
pub fn spawn_reclaimers(
    exchange: &Arc<ProcessorExchange>,
    endpoints: &Arc<LinuxVirtualEndpointBackend>,
) -> Result<ResourceReclaimer, String> {
    let weak_exchange = Arc::downgrade(exchange);
    let weak_endpoints = Arc::downgrade(endpoints);
    ResourceReclaimer::start("mixrelay-linux-reclaimer", move || {
        let Some(exchange) = weak_exchange.upgrade() else {
            return false;
        };
        exchange.reclaim_retired();
        drop(exchange);
        let Some(endpoints) = weak_endpoints.upgrade() else {
            return false;
        };
        endpoints.reclaim_retired();
        true
    })
    .map_err(|error| format!("could not start Linux resource reclaimer: {error}"))
}

/// The engine's startup render format.
pub(crate) const fn render_format() -> RenderFormat {
    RenderFormat {
        sample_rate: ENGINE_SAMPLE_RATE,
        block_frames: ENGINE_BLOCK_FRAMES,
    }
}

pub(crate) const fn native_format(format: RenderFormat) -> mixrelay_linux_pipewire::StreamFormat {
    mixrelay_linux_pipewire::StreamFormat {
        sample_rate: format.sample_rate,
        block_frames: format.block_frames,
    }
}

/// `PipeWire` node name for a graph-owned stream. Graph IDs are user data, so every
/// character outside the node-name alphabet becomes `_`.
pub(crate) fn sanitize_node_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("io.mixrelay.{value}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_names_keep_the_pipewire_alphabet_and_replace_everything_else() {
        assert_eq!(
            sanitize_node_name("mic-1.left_A"),
            "io.mixrelay.mic-1.left_A"
        );
        assert_eq!(
            sanitize_node_name("app capture/ü:1"),
            "io.mixrelay.app_capture___1"
        );
    }
}
