//! Runs a document through the real backend, processing state and graph driver.
use super::*;

pub(super) fn with_engine_graph<R>(
    document: &GraphDocument,
    format: RenderFormat,
    playback_intents: &[mixrelay_audio_graph::composition::ApplicationPlaybackIntent],
    run: impl FnOnce(
        &mixrelay_linux_pipewire::GraphDriver<LinuxGraphProcess<'_>>,
        (
            &mixrelay_engine_core::session::AdapterRuntimeStatus,
            &DriverCounters,
        ),
        &crate::backend::LinuxVirtualEndpointBackend,
        &Arc<mixrelay_render::ProcessorExchange>,
        &Arc<mixrelay_render::RuntimeControlBank>,
    ) -> R,
) -> (R, DriverStats) {
    let backend = crate::backend::LinuxVirtualEndpointBackend::new(
        crate::test_support::test_driver(),
        format,
        Arc::new(crate::format::EndpointObservation::default()),
        mixrelay_time::StdClock::new(),
    );
    let processor = crate::runtime::create_initial_processor(
        document,
        mixrelay_engine_core::default_peak_meter_bank(),
        format,
    )
    .expect("compile exact ApplicationCapture to AudioDeviceOutput graph");
    let exchange = Arc::new(mixrelay_render::ProcessorExchange::new(
        processor.plan().revision(),
    ));
    let controls = Arc::clone(processor.plan().controls());
    let playback = if playback_intents.is_empty() {
        mixrelay_engine_core::PlaybackBindings::Preserve
    } else {
        mixrelay_engine_core::PlaybackBindings::Forward {
            intents: playback_intents,
            controls: &controls,
        }
    };
    backend
        .reconcile(document, playback)
        .expect("stage graph resources");
    let status = active_runtime_status(format);
    let state =
        crate::processing::LinuxProcessState::prepared(&backend, processor, &exchange, &status);
    let processing = status
        .begin_processing()
        .expect("start engine processing lease");
    let counters = Arc::new(DriverCounters {
        clock_diagnostics: Arc::clone(state.clock_diagnostics()),
        ..DriverCounters::default()
    });
    let driver = mixrelay_linux_pipewire::GraphDriver::open(
        LinuxGraphProcess {
            state,
            counters: Arc::clone(&counters),
        },
        mixrelay_linux_pipewire::DriverFormat::new(format.sample_rate, format.block_frames)
            .expect("valid fixed graph-driver format"),
    )
    .expect("open real Linux GraphDriver");
    let result = run(
        &driver,
        (&status, &counters),
        &backend,
        &exchange,
        &controls,
    );
    drop(driver);
    drop(processing);
    (result, read_driver_counters(&counters))
}

pub(super) fn active_runtime_status(
    format: RenderFormat,
) -> Arc<mixrelay_engine_core::session::AdapterRuntimeStatus> {
    let status = Arc::new(mixrelay_engine_core::session::AdapterRuntimeStatus::new(
        crate::runtime::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    ));
    status.activate_format(format);
    status
}

pub(super) fn open_input_capture(
    process_id: u32,
    source_serial: &str,
) -> (CaptureSet, CaptureWatch) {
    let request = application_capture_request(process_id);
    let format = RenderFormat {
        sample_rate: SAMPLE_RATE,
        block_frames: BLOCK_FRAMES,
    };
    let (mut captures, mut watch) = CaptureSet::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &[(CAPTURE_NODE.into(), request)],
        1,
        format,
        None,
    )
    .expect("open fixed-format input evidence tap");
    super::recover_until(&mut captures, &mut watch, CAPTURE_NODE, true);
    assert_eq!(watch.entries.len(), 1);
    assert_eq!(watch.entries[0].serials, [source_serial]);
    (captures, watch)
}
