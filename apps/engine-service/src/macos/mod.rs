//! Composes native macOS owners with the engine and control adapters.
mod endpoint_driver;
mod latency_publication;
mod maintenance;
mod shutdown;
use mixrelay_audio_graph::RenderFormat;
use mixrelay_engine_core::{default_peak_meter_bank, session::AdapterRuntimeStatus};
use mixrelay_engine_host::{
    ShutdownSignals, crash_guard::CrashGuard, persistence::config_directory,
    session_preferences::SessionPreferences,
};
use mixrelay_engine_service::{
    EngineGraphController, EngineGraphControllerConfig, control_service::ControlService,
};
use mixrelay_macos_engine::MacVirtualEndpointBackend;
use mixrelay_macos_engine::{
    MacDevices, RuntimeLatencyReporter, create_initial_processor, endpoint_service,
    start_audio_worker,
};
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_render::ProcessorExchange;
use std::sync::Arc;
fn prepare_runtime() -> (Arc<AdapterRuntimeStatus>, RenderFormat) {
    let mut status =
        mixrelay_macos_engine::prepare_runtime(Arc::new(mixrelay_time::StdClock::new()));
    let store =
        SessionPreferences::for_backend(mixrelay_audio_io::session::AudioBackendKind::CoreAudio)
            .map(|store| {
                Arc::new(store)
                    as Arc<dyn mixrelay_engine_core::session_settings::SessionPreferences>
            })
            .map_err(|error| error.to_string());
    let format = status.restore_preferences(store);
    (Arc::new(status), format)
}
pub(super) async fn run() -> Result<(), String> {
    use mixrelay_macos_transport::app_lifecycle::{CONTROLLER_LEASE, ENGINE_LEASE, ProcessLease};
    let Some(engine_lease) =
        ProcessLease::acquire_for_controller(ENGINE_LEASE, CONTROLLER_LEASE)
            .map_err(|error| format!("could not claim engine lifecycle: {error}"))?
    else {
        tracing::info!("controller is absent; refusing demand-started audio execution");
        return Ok(());
    };
    engine_lease
        .record("running")
        .map_err(|error| format!("engine lifecycle record failed: {error}"))?;
    // Stops are caught before anything can fail, and the crash guard covers the whole run.
    let signals = ShutdownSignals::register()
        .map_err(|error| format!("could not observe shutdown requests: {error}"))?;
    let (guard, tripped) = CrashGuard::arm(config_directory());
    let result = run_owned(signals, tripped).await;
    endpoint_service::stop_virtual_sources();
    let retired = MacDevices::finish_retirement();
    // A killed controller cannot run its own Quit handler. Its OS-owned lease
    // still releases, so the surviving engine finalizes recordings and then
    // shuts down the independent HAL inventory, preserving the stopped state.
    let result = shutdown::finish(result, retired, async {
        mixrelay_driver_adapters::DriverClient::new()
            .map_err(|error| error.to_string())?
            .set_active(false)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await;
    let outcome = result
        .as_ref()
        .map_or_else(|error| format!("failed\n{error}"), |()| "stopped".into());
    engine_lease
        .record(&outcome)
        .map_err(|error| format!("engine exit record failed: {error}"))?;
    if result.is_ok()
        && let Some(guard) = guard
    {
        guard.finish();
    }
    result
}

#[expect(
    clippy::too_many_lines,
    reason = "startup and retirement retain the native owners in one explicit lifetime order"
)]
async fn run_owned(signals: ShutdownSignals, repeated_failures: bool) -> Result<(), String> {
    // Reserve the control endpoint before native publication or audio ownership.
    // A second service must fail without preparing devices beside the running one.
    let service = ControlService::bind(mixrelay_local_socket::socket_directory())
        .await
        .map_err(|error| error.to_string())?;
    initialize_native_transport()?;
    let (runtime_status, format) = prepare_runtime();
    let meters = default_peak_meter_bank();
    let initial_document = mixrelay_audio_graph::GraphDocument::new(1, Vec::new(), Vec::new());
    let processor = create_initial_processor(&initial_document, format, Arc::clone(&meters))?;
    let exchange = Arc::new(ProcessorExchange::new(processor.plan().revision()));
    let latency =
        mixrelay_engine_host::latency_maintenance::Observer::start(&exchange, processor.plan())?;
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
        EngineGraphControllerConfig {
            clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
            format,
            meters,
            initial_bindings: processor.plan().meter_bindings().to_vec(),
            endpoint_backend: endpoint_backend.clone(),
            required_endpoint_node_id: None,
            processor_catalog: ProcessorCatalog::discover(),
            graph_resources: devices.clone(),
            runtime_status: devices.clone(),
        },
        initial_document,
    );
    // Saved device references are resolved through Driver's blocking control API.
    // Recall must finish before audio starts, but outside the Tokio IO executor.
    let controller = restore_controller(controller, repeated_failures).await?;
    // Declared before AudioWorker so reclamation remains available until the
    // audio owner has stopped and joined, including partial-startup failures.
    let reclaimer = mixrelay_macos_engine::start_reclaimer(
        Arc::downgrade(&exchange),
        Arc::downgrade(&devices),
    )?;
    let runtime_latency = RuntimeLatencyReporter::start(
        Arc::clone(&exchange),
        Arc::clone(&devices),
        mixrelay_control_server::ControlBackend::incarnation(controller.as_ref()),
        Arc::new(
            latency_publication::DriverLatencyPublication::new()
                .map_err(|error| error.to_string())?,
        ),
    );
    let (worker, mut completion) =
        start_audio_worker(processor, exchange, devices.clone(), runtime_status.clone())
            .map_err(|error| format!("could not start audio worker: {error}"))?;
    let observers = maintenance::ControlObservers::start(
        &runtime_status,
        &controller,
        &devices,
        &endpoint_backend,
    )?;
    let serving = service
        .serve(
            controller,
            completion.shutdown(stop_requested(signals)),
            || {
                observers.stop_observing();
            },
        )
        .await
        .map_err(|error| error.to_string());
    let observing = observers.finish();
    let joined = worker.finish();
    let completed = completion.finish().await.map_err(|error| error.to_string());
    let processor_cleanup = reclaimer.finish();
    let latency_stopped = latency.finish();
    let reporting_stopped = runtime_latency.finish().await;
    shutdown::collect([
        ("control", serving),
        ("observers", observing),
        ("audio_join", joined),
        ("audio_result", completed),
        ("reclaimer", processor_cleanup),
        ("latency_observer", latency_stopped),
        ("runtime_latency", reporting_stopped),
    ])
}

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

/// An OS shutdown request or the controller releasing the engine.
async fn stop_requested(signals: ShutdownSignals) -> std::io::Result<()> {
    tokio::select! {
        result = signals.requested() => result,
        () = controller_released() => Ok(()),
    }
}

async fn controller_released() {
    shutdown::wait_for_controller(mixrelay_macos_transport::app_lifecycle::CONTROLLER_LEASE).await;
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
