//! Composes native macOS owners with the engine and control adapters.
mod endpoint_driver;
mod latency_publication;
mod maintenance;
mod shutdown;
use crate::{
    control_service::ControlService,
    controller::{EngineGraphController, EngineGraphControllerConfig},
};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::session::AudioBackendKind;
use mixrelay_engine_core::{default_peak_meter_bank, session::AdapterRuntimeStatus};
use mixrelay_engine_host::{
    ShutdownSignals, crash_guard::CrashGuard, latency_maintenance, persistence::config_directory,
};
use mixrelay_macos_engine::{
    MacDevices, MacVirtualEndpointBackend, RuntimeLatencyReporter, create_initial_processor,
    endpoint_service, start_audio_worker,
};
use mixrelay_macos_transport::app_lifecycle::{CONTROLLER_LEASE, ENGINE_LEASE, ProcessLease};
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_render::{BlockProcessor, PeakMeterBank, ProcessorExchange};
use std::sync::Arc;

/// Runs the macOS engine service while the controller owns the engine lifecycle.
///
/// # Errors
/// Returns lifecycle, native startup, serving or retirement failure.
pub async fn run() -> Result<(), String> {
    let Some(engine_lease) = claim_engine_lease()? else {
        tracing::info!("controller is absent; refusing demand-started audio execution");
        return Ok(());
    };
    // Stops are caught before anything can fail, and the crash guard covers the whole run.
    let signals = ShutdownSignals::register()
        .map_err(|error| format!("could not observe shutdown requests: {error}"))?;
    let (guard, tripped) = CrashGuard::arm(config_directory());
    let result = run_owned(signals, tripped).await;
    let result = retire_native_audio(result).await;
    engine_lease
        .record(&shutdown::exit_record(&result))
        .map_err(|error| format!("engine exit record failed: {error}"))?;
    crate::shutdown::finish_crash_guard(guard, &result);
    result
}

/// The engine runs only beside a controller that owns its lifecycle; `None` means the
/// controller is absent.
fn claim_engine_lease() -> Result<Option<ProcessLease>, String> {
    let Some(engine_lease) =
        ProcessLease::acquire_for_controller(ENGINE_LEASE, CONTROLLER_LEASE)
            .map_err(|error| format!("could not claim engine lifecycle: {error}"))?
    else {
        return Ok(None);
    };
    engine_lease
        .record("running")
        .map_err(|error| format!("engine lifecycle record failed: {error}"))?;
    Ok(Some(engine_lease))
}

/// A killed controller cannot run its own Quit handler. Its OS-owned lease still
/// releases, so the surviving engine finalizes recordings and then shuts down the
/// independent HAL inventory, preserving the stopped state.
async fn retire_native_audio(run: Result<(), String>) -> Result<(), String> {
    endpoint_service::stop_virtual_sources();
    let retired = MacDevices::finish_retirement();
    shutdown::finish(run, retired, stop_driver_devices()).await
}

async fn stop_driver_devices() -> Result<(), String> {
    mixrelay_driver_adapters::DriverClient::new()
        .map_err(|error| error.to_string())?
        .set_active(false)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn run_owned(signals: ShutdownSignals, repeated_failures: bool) -> Result<(), String> {
    // Reserve the control endpoint before native publication or audio ownership.
    // A second service must fail without preparing devices beside the running one.
    let service = ControlService::bind(mixrelay_local_socket::socket_directory())
        .await
        .map_err(|error| error.to_string())?;
    initialize_native_transport()?;
    let (runtime_status, format) = prepare_runtime();
    let meters = default_peak_meter_bank();
    let initial_document = GraphDocument::new(1, Vec::new(), Vec::new());
    let processor = create_initial_processor(&initial_document, format, Arc::clone(&meters))?;
    let exchange = Arc::new(ProcessorExchange::new(processor.plan().revision()));
    let latency = latency_maintenance::Observer::start(&exchange, processor.plan())?;
    let endpoint_backend = Arc::new(MacVirtualEndpointBackend::new(
        mixrelay_audio_io::formats::SAMPLE_RATES.to_vec(),
        Arc::new(endpoint_driver::DriverEndpointControl::new()),
    ));
    let devices = Arc::new(MacDevices::new(
        runtime_status.clone(),
        format.sample_rate,
        format.block_frames,
    ));
    let controller = EngineGraphController::new_with_initial_document(
        Arc::clone(&exchange),
        controller_config(&processor, format, meters, &endpoint_backend, &devices),
        initial_document,
    );
    let controller = restore_controller(controller, repeated_failures).await?;
    // Declared before AudioWorker so reclamation remains available until the
    // audio owner has stopped and joined, including partial-startup failures.
    let reclaimer = mixrelay_macos_engine::start_reclaimer(
        Arc::downgrade(&exchange),
        Arc::downgrade(&devices),
    )?;
    let runtime_latency = start_runtime_latency(&exchange, &devices, &controller)?;
    let (worker, mut completion) =
        start_audio_worker(processor, exchange, devices.clone(), runtime_status.clone())
            .map_err(|error| format!("could not start audio worker: {error}"))?;
    let observers = maintenance::ControlObservers::start(
        &runtime_status,
        &controller,
        &devices,
        &endpoint_backend,
    )?;
    let stop = shutdown::first_stop(
        signals.requested(),
        shutdown::wait_for_controller(CONTROLLER_LEASE),
    );
    let serving = service
        .serve(controller, completion.shutdown(stop), || {
            observers.stop_observing();
        })
        .await
        .map_err(|error| error.to_string());
    // Each owner retires in this order whatever the earlier ones returned.
    let observing = observers.finish();
    let joined = worker.finish();
    let completed = completion.finish().await.map_err(|error| error.to_string());
    let processor_cleanup = reclaimer.finish();
    let latency_stopped = latency.finish();
    let reporting_stopped = runtime_latency.finish().await;
    crate::shutdown::collect([
        ("control", serving),
        ("observers", observing),
        ("audio_join", joined),
        ("audio_result", completed),
        ("reclaimer", processor_cleanup),
        ("latency_observer", latency_stopped),
        ("runtime_latency", reporting_stopped),
    ])
}

fn initialize_native_transport() -> Result<(), String> {
    let exit_code = endpoint_service::start(
        Arc::new(mixrelay_time::StdClock::new()),
        mixrelay_realtime::Timebase::current(),
    );
    if exit_code != 0 {
        return Err(format!(
            "native engine service initialization failed with code {exit_code}"
        ));
    }
    Ok(())
}

fn prepare_runtime() -> (Arc<AdapterRuntimeStatus>, RenderFormat) {
    let mut status =
        mixrelay_macos_engine::prepare_runtime(Arc::new(mixrelay_time::StdClock::new()));
    let format = status.restore_preferences(crate::startup::saved_session_preferences(
        AudioBackendKind::CoreAudio,
    ));
    (Arc::new(status), format)
}

/// `MacDevices` owns both the graph's hardware resources and the runtime status the
/// controller reports.
fn controller_config(
    initial: &BlockProcessor,
    format: RenderFormat,
    meters: Arc<PeakMeterBank>,
    endpoint_backend: &Arc<MacVirtualEndpointBackend>,
    devices: &Arc<MacDevices>,
) -> EngineGraphControllerConfig {
    EngineGraphControllerConfig {
        clock: Arc::new(mixrelay_time::StdClock::new()),
        format,
        meters,
        initial_bindings: initial.plan().meter_bindings().to_vec(),
        endpoint_backend: endpoint_backend.clone(),
        required_endpoint_node_id: None,
        processor_catalog: ProcessorCatalog::discover(),
        graph_resources: devices.clone(),
        runtime_status: devices.clone(),
    }
}

/// Saved device references are resolved through Driver's blocking control API.
/// Recall must finish before audio starts, but outside the Tokio IO executor.
async fn restore_controller(
    controller: EngineGraphController,
    repeated_failures: bool,
) -> Result<Arc<EngineGraphController>, String> {
    tokio::task::spawn_blocking(move || {
        Arc::new(
            controller
                .after_repeated_failures(repeated_failures)
                .with_graph_catalog(),
        )
    })
    .await
    .map_err(|error| format!("could not restore graph catalog: {error}"))
}

fn start_runtime_latency(
    exchange: &Arc<ProcessorExchange>,
    devices: &Arc<MacDevices>,
    controller: &EngineGraphController,
) -> Result<RuntimeLatencyReporter, String> {
    let publication =
        latency_publication::DriverLatencyPublication::new().map_err(|error| error.to_string())?;
    Ok(RuntimeLatencyReporter::start(
        Arc::clone(exchange),
        Arc::clone(devices),
        mixrelay_control_server::ControlBackend::incarnation(controller),
        Arc::new(publication),
    ))
}

#[cfg(test)]
mod tests;
