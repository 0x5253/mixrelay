use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 96_000,
    block_frames: 256,
};

#[derive(Default)]
struct NativeCalls {
    status: AtomicUsize,
    preparation: AtomicUsize,
}

impl RuntimeStatusBackend for NativeCalls {
    fn status(&self, format: RenderFormat) -> AppStatus {
        self.status.fetch_add(1, Ordering::Relaxed);
        FixedRuntimeStatus::new(DriverState::Ready).status(format)
    }
}

impl ProcessingControlBackend for NativeCalls {}

impl SessionFormatBackend for NativeCalls {}

impl GraphResourceBackend for NativeCalls {
    fn reconcile(
        &self,
        _document: &GraphDocument,
        _: crate::PlaybackBindings<'_>,
    ) -> Result<(), HostAudioBackendError> {
        self.preparation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn configuration(native: &Arc<NativeCalls>) -> EngineGraphControllerConfig {
    EngineGraphControllerConfig {
        clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        format: FORMAT,
        initial_bindings: Vec::new(),
        endpoint_backend: Arc::new(ContractOnlyVirtualEndpointBackend::default()),
        required_endpoint_node_id: None,
        graph_resources: native.clone(),
        runtime_status: native.clone(),

        ..crate::test_support::configuration(FORMAT)
    }
}

#[test]
fn explicit_startup_preserves_prepared_state_without_querying_or_preparing_native_io() {
    let document = GraphDocument::new(
        7,
        vec![NodeSpec {
            control: mixrelay_audio_graph::NodeControl::default(),
            id: NodeId::from("gain"),
            label: Some("Prepared gain".into()),
            config: NodeConfig::Gain(GainConfig {
                channels: 2,
                gain_db: -6.0,
            }),
        }],
        Vec::new(),
    );
    let execution = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let plan = RenderPlan::prepare(&execution).unwrap();
    let native = Arc::new(NativeCalls::default());
    let mut config = configuration(&native);
    config.initial_bindings = plan.meter_bindings().to_vec();
    let meters = config.meters.clone();
    let exchange = Arc::new(ProcessorExchange::new(plan.revision()));

    let controller = EngineGraphController::new_with_initial_document(
        exchange.clone(),
        config,
        document.clone(),
    );

    assert_eq!(controller.graph(), document);
    assert_eq!(controller.context.read().format, FORMAT);
    assert!(Arc::ptr_eq(&controller.exchange, &exchange));
    assert!(Arc::ptr_eq(&controller.meters, &meters));
    let bindings = controller.meter_bindings.read();
    assert_eq!(bindings.len(), 1);
    assert!(!bindings[&7].is_empty());
    assert_eq!(bindings[&7].len(), plan.meter_bindings().len());
    for (actual, expected) in bindings[&7].iter().zip(plan.meter_bindings()) {
        assert_eq!(actual.node_id(), expected.node_id());
        assert_eq!(actual.port_id(), expected.port_id());
        assert_eq!(actual.channels(), expected.channels());
    }
    assert!(controller.execution_views.read().is_empty());
    assert_eq!(native.status.load(Ordering::Relaxed), 0);
    assert_eq!(native.preparation.load(Ordering::Relaxed), 0);
    assert_eq!(exchange.revisions().submitted, 7);
}

#[test]
fn default_startup_selects_its_document_once_without_preparing_native_io() {
    let native = Arc::new(NativeCalls::default());
    let controller = EngineGraphController::new_with_configuration(
        Arc::new(ProcessorExchange::new(3)),
        configuration(&native),
    );

    assert_eq!(controller.graph(), default_session_document(3, FORMAT));
    assert_eq!(native.status.load(Ordering::Relaxed), 1);
    assert_eq!(native.preparation.load(Ordering::Relaxed), 0);
}

#[test]
fn mismatched_startup_revision_fails_before_native_observation() {
    let native = Arc::new(NativeCalls::default());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        EngineGraphController::new_with_initial_document(
            Arc::new(ProcessorExchange::new(1)),
            configuration(&native),
            GraphDocument::new(2, Vec::new(), Vec::new()),
        )
    }));

    assert!(result.is_err());
    assert_eq!(native.status.load(Ordering::Relaxed), 0);
    assert_eq!(native.preparation.load(Ordering::Relaxed), 0);
}
