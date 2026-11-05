//! Composes `PipeWire` native owners with engine, processor, recorder and control adapters.
use crate::{
    control_service::ControlService,
    controller::{EngineGraphController, EngineGraphControllerConfig},
};
use mixrelay_audio_graph::{GraphDocument, RenderFormat};
use mixrelay_audio_io::session::AudioBackendKind;
use mixrelay_engine_core::{
    default_peak_meter_bank, session::AdapterRuntimeStatus, worker_completion::Completion,
};
use mixrelay_engine_host::{
    AudioWorker, ShutdownSignals, crash_guard::CrashGuard, latency_maintenance,
    persistence::config_directory,
};
use mixrelay_linux_engine::{
    EndpointObservation, FormatMonitor, LinuxVirtualEndpointBackend, WorkerError,
    create_initial_processor, run_audio_worker, spawn_reclaimers, start_recovery,
};
use mixrelay_processor_host::ProcessorCatalog;
use mixrelay_render::{BlockProcessor, PeakMeterBank, ProcessorExchange};
use std::{sync::Arc, thread};
mod driver;
/// Runs the Linux engine service, optionally owned by a supervising controller.
///
/// # Errors
/// Returns admission, native startup, serving or retirement failure.
pub fn run(owner: Option<mixrelay_engine_supervisor::EngineOwner>) -> Result<(), String> {
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
    crate::shutdown::finish_crash_guard(guard, &result);
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
    let initial_format = runtime_status.restore_preferences(
        crate::startup::saved_session_preferences(AudioBackendKind::PipeWire),
    );
    let driver = Arc::new(driver::DriverEndpointControl::new());
    let endpoint_backend = LinuxVirtualEndpointBackend::new(
        driver.clone(),
        initial_format,
        observation,
        mixrelay_time::StdClock::new(),
    );
    let runtime_status = Arc::new(runtime_status);
    let meters = default_peak_meter_bank();
    let initial_document = GraphDocument::new(1, Vec::new(), Vec::new());
    let processor =
        create_initial_processor(&initial_document, Arc::clone(&meters), initial_format)?;
    let exchange = Arc::new(ProcessorExchange::new(processor.plan().revision()));
    let latency = latency_maintenance::Observer::start(&exchange, processor.plan())?;
    let controller = Arc::new(
        EngineGraphController::new_with_initial_document(
            Arc::clone(&exchange),
            controller_config(
                &processor,
                initial_format,
                meters,
                &endpoint_backend,
                &runtime_status,
            ),
            initial_document,
        )
        .after_repeated_failures(repeated_failures)
        .with_graph_catalog(),
    );

    let reclaimer = spawn_reclaimers(&exchange, &endpoint_backend)?;
    let recovery = start_recovery(&endpoint_backend, &exchange)?;
    let (worker, mut completion) =
        start_audio_worker(processor, exchange, endpoint_backend, &runtime_status)?;
    let format_monitor = start_format_monitor(&runtime_status, driver, &controller)?;
    let maintenance = controller.start_control_maintenance()?;

    let stop = mixrelay_engine_supervisor::shutdown(owner, signals.requested());
    let result = runtime
        .block_on(service.serve(controller, completion.shutdown(stop), || {
            format_monitor.stop_observing();
            maintenance.stop_observing();
        }))
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

/// The `PipeWire` endpoint backend also owns the graph's hardware resources.
fn controller_config(
    initial: &BlockProcessor,
    format: RenderFormat,
    meters: Arc<PeakMeterBank>,
    endpoint_backend: &Arc<LinuxVirtualEndpointBackend>,
    runtime_status: &Arc<AdapterRuntimeStatus>,
) -> EngineGraphControllerConfig {
    EngineGraphControllerConfig {
        clock: Arc::new(mixrelay_time::StdClock::new()),
        format,
        meters,
        initial_bindings: initial.plan().meter_bindings().to_vec(),
        endpoint_backend: endpoint_backend.clone(),
        required_endpoint_node_id: None,
        processor_catalog: ProcessorCatalog::discover(),
        graph_resources: endpoint_backend.clone(),
        runtime_status: runtime_status.clone(),
    }
}

/// Runs `PipeWire` processing on its own thread, joined by the returned `AudioWorker`.
fn start_audio_worker(
    processor: Box<BlockProcessor>,
    exchange: Arc<ProcessorExchange>,
    endpoint_backend: Arc<LinuxVirtualEndpointBackend>,
    runtime_status: &Arc<AdapterRuntimeStatus>,
) -> Result<(AudioWorker, Completion<WorkerError>), String> {
    let worker_status = runtime_status.clone();
    let (worker_result, completion) = mixrelay_engine_core::worker_completion::channel();
    let audio_thread = thread::Builder::new()
        .name("mixrelay-pipewire-audio".to_owned())
        .spawn(move || {
            let result = run_audio_worker(processor, exchange, &endpoint_backend, &worker_status);
            let _ = worker_result.send(result);
        })
        .map_err(|error| format!("could not start PipeWire audio worker: {error}"))?;
    Ok((
        AudioWorker::new(runtime_status.clone(), audio_thread),
        completion,
    ))
}

fn start_format_monitor(
    runtime_status: &Arc<AdapterRuntimeStatus>,
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
