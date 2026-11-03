//! Composes `PipeWire` native owners with engine, processor, recorder and control adapters.
use mixrelay_audio_graph::GraphDocument;
use mixrelay_engine_core::default_peak_meter_bank;
use mixrelay_engine_host::{
    ShutdownSignals, crash_guard::CrashGuard, persistence::config_directory,
    session_preferences::SessionPreferences,
};
use mixrelay_engine_service::{
    EngineGraphController, EngineGraphControllerConfig, control_service::ControlService,
};
use mixrelay_linux_engine::{
    EndpointObservation, FormatMonitor, LinuxVirtualEndpointBackend, create_initial_processor,
    run_audio_worker, spawn_reclaimers, start_recovery,
};
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_render::ProcessorExchange;
use std::{sync::Arc, thread};
mod driver;
pub(super) fn run(owner: Option<mixrelay_engine_supervisor::EngineOwner>) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("could not initialize Linux async runtime: {error}"))?;
    // Admission precedes publishing PipeWire endpoints or starting audio owners.
    let service = runtime
        .block_on(ControlService::bind(
            mixrelay_local_socket::socket_directory(),
        ))
        .map_err(|error| error.to_string())?;
    // Stops are caught before anything can fail, and the crash guard covers the whole run.
    let signals = runtime
        .block_on(async { ShutdownSignals::register() })
        .map_err(|error| format!("could not observe shutdown requests: {error}"))?;
    let (guard, tripped) = CrashGuard::arm(config_directory());
    let result = run_bound(&runtime, service, owner, signals, tripped);
    if result.is_ok()
        && let Some(guard) = guard
    {
        guard.finish();
    }
    result
}

fn run_bound(
    runtime: &tokio::runtime::Runtime,
    service: ControlService,
    owner: Option<mixrelay_engine_supervisor::EngineOwner>,
    signals: ShutdownSignals,
    repeated_failures: bool,
) -> Result<(), String> {
    let observation = Arc::new(EndpointObservation::default());
    let mut runtime_status = mixrelay_linux_engine::prepare_runtime(
        Arc::new(mixrelay_time::StdClock::new()),
        Arc::clone(&observation),
    );
    let initial_format = runtime_status.restore_preferences(session_preferences());
    let driver = Arc::new(driver::DriverEndpointControl::new());
    let endpoint_backend =
        LinuxVirtualEndpointBackend::new(driver.clone(), initial_format, observation);
    let runtime_status = Arc::new(runtime_status);
    let meters = default_peak_meter_bank();
    let initial_document = GraphDocument::new(1, Vec::new(), Vec::new());
    let processor =
        create_initial_processor(&initial_document, Arc::clone(&meters), initial_format)?;
    let exchange = Arc::new(ProcessorExchange::new(processor.plan().revision()));
    let latency =
        mixrelay_engine_host::latency_maintenance::Observer::start(&exchange, processor.plan())?;
    let controller = Arc::new(
        EngineGraphController::new_with_initial_document(
            Arc::clone(&exchange),
            EngineGraphControllerConfig {
                clock: std::sync::Arc::new(mixrelay_time::StdClock::new()),
                format: initial_format,
                meters,
                initial_bindings: processor.plan().meter_bindings().to_vec(),
                endpoint_backend: endpoint_backend.clone(),
                required_endpoint_node_id: None,
                processor_catalog: ProcessorCatalog::discover(),
                graph_resources: endpoint_backend.clone(),
                runtime_status: runtime_status.clone(),
            },
            initial_document,
        )
        .after_repeated_failures(repeated_failures)
        .with_graph_catalog(),
    );

    let reclaimer = spawn_reclaimers(&exchange, &endpoint_backend)?;
    let recovery = start_recovery(&endpoint_backend, &exchange)?;
    let worker_status = runtime_status.clone();
    let (worker_result, mut completion) = mixrelay_engine_core::worker_completion::channel();
    let audio_thread = thread::Builder::new()
        .name("mixrelay-pipewire-audio".to_owned())
        .spawn(move || {
            let result = run_audio_worker(processor, exchange, &endpoint_backend, &worker_status);
            let _ = worker_result.send(result);
        })
        .map_err(|error| format!("could not start PipeWire audio worker: {error}"))?;
    let worker = mixrelay_engine_host::AudioWorker::new(runtime_status.clone(), audio_thread);
    let format_monitor = start_format_monitor(&runtime_status, driver, &controller)?;
    let maintenance = controller.start_control_maintenance()?;

    let result = runtime
        .block_on(service.serve(
            controller,
            completion.shutdown(mixrelay_engine_supervisor::shutdown(
                owner,
                signals.requested(),
            )),
            || {
                format_monitor.stop_observing();
                maintenance.stop_observing();
            },
        ))
        .map_err(|error| error.to_string());
    // Evaluate every retirement in ownership order, retaining all failures.
    crate::shutdown::collect([
        ("Control service", result),
        ("Format monitor", format_monitor.finish()),
        ("Control maintenance", maintenance.finish()),
        ("Audio worker join", worker.finish()),
        (
            "Audio processing",
            runtime
                .block_on(completion.finish())
                .map_err(|error| error.to_string()),
        ),
        ("Resource recovery", recovery.finish()),
        ("Resource retirement", reclaimer.finish()),
        ("Latency observer", latency.finish()),
    ])
}

fn start_format_monitor(
    runtime_status: &Arc<mixrelay_engine_core::session::AdapterRuntimeStatus>,
    driver: Arc<driver::DriverEndpointControl>,
    controller: &Arc<EngineGraphController>,
) -> Result<FormatMonitor, String> {
    let observed_controller = Arc::downgrade(controller);
    let mut last_error = None;
    FormatMonitor::start(runtime_status, driver, move || {
        let Some(controller) = observed_controller.upgrade() else {
            return false;
        };
        let error = controller.reconcile_observed_device_rate().err();
        if error != last_error {
            if let Some(error) = &error {
                tracing::warn!(?error, "MixRelay endpoint format follow failed");
            }
            last_error = error;
        }
        true
    })
}

fn session_preferences()
-> Result<Arc<dyn mixrelay_engine_core::session_settings::SessionPreferences>, String> {
    SessionPreferences::for_backend(mixrelay_audio_io::session::AudioBackendKind::PipeWire)
        .map(|store| {
            Arc::new(store) as Arc<dyn mixrelay_engine_core::session_settings::SessionPreferences>
        })
        .map_err(|error| error.to_string())
}
