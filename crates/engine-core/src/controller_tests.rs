//! Controller startup, plan admission and publication regression coverage.

use super::*;
use mixrelay_audio_graph::ExecutionPlan;
use mixrelay_render::BlockProcessor;
use mixrelay_render::{RenderPlan, RenderPlanError};

#[test]
fn explicit_empty_startup_does_not_invent_a_default_endpoint() {
    let controller = EngineGraphController::new_with_initial_document(
        Arc::new(ProcessorExchange::new(1)),
        EngineGraphControllerConfig {
            clock: std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            format: FORMAT,
            initial_bindings: vec![],
            endpoint_backend: Arc::new(ContractOnlyVirtualEndpointBackend::default()),
            required_endpoint_node_id: None,
            graph_resources: Arc::new(NoopGraphResourceBackend),
            runtime_status: Arc::new(FixedRuntimeStatus::new(DriverState::Ready)),
            // Contract-only backends route no OS audio; these fixtures keep the
            // unrestricted engine contract.
            ..crate::test_support::configuration(FORMAT)
        },
        GraphDocument::new(1, vec![], vec![]),
    );
    assert!(controller.graph().nodes.is_empty());
    assert_eq!(controller.graph_status().active_revision, 1);
}

const FORMAT: RenderFormat = RenderFormat {
    sample_rate: 48_000,
    block_frames: 128,
};

fn fail_render(_execution: &ExecutionPlan) -> Result<RenderPlan, RenderPlanError> {
    Err(RenderPlanError::BufferSizeOverflow)
}

fn controller(exchange: Arc<ProcessorExchange>) -> EngineGraphController {
    crate::test_support::controller(
        exchange,
        DEFAULT_ENDPOINT_NODE_ID,
        FORMAT,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    )
}

fn processor(document: &GraphDocument) -> BlockProcessor {
    let engine = controller(Arc::new(ProcessorExchange::new(document.revision)));
    *engine
        .prepare_and_validate(
            document,
            CompileContext {
                format: FORMAT,
                limits: CompileLimits::default(),
            },
        )
        .expect("test graph should compose and prepare")
        .processor
}

#[test]
fn builds_direct_and_gain_use_case_documents() {
    let direct = default_duplex_document(1, None);
    assert_eq!(direct.nodes.len(), 1);
    assert_eq!(direct.edges.len(), 2);
    let gain = default_duplex_document(2, Some(-6.0));
    assert_eq!(gain.nodes.len(), 2);
    assert_eq!(gain.edges.len(), 4);
}

#[test]
fn exposes_runtime_status_and_submits_a_valid_gain_graph() {
    let exchange = Arc::new(ProcessorExchange::new(1));
    let controller = controller(Arc::clone(&exchange));
    assert_eq!(controller.status().driver_state, DriverState::Ready);
    assert_eq!(controller.status().sample_rate, Some(48_000));
    assert_eq!(controller.status().buffer_frames, Some(128));
    assert_eq!(controller.graph_status().active_revision, 1);

    let response = controller
        .apply_graph_request(default_duplex_document(2, Some(-6.020_600_3)).into())
        .expect("gain graph should submit");
    assert_eq!(response.revision, 2);
    assert_eq!(response.state, GraphApplyState::Pending);
    assert_eq!(controller.graph_status().submitted_revision, 2);
    assert_eq!(controller.graph().revision, 2);

    let mut current = Box::new(processor(&default_duplex_document(1, None)));
    assert!(exchange.activate_pending(&mut current));
    assert_eq!(current.plan().revision(), 2);
    assert_eq!(controller.graph_status().active_revision, 2);
}

#[test]
fn rejects_stale_invalid_and_unsupported_graphs_without_replacing_audio() {
    let exchange = Arc::new(ProcessorExchange::new(3));
    let controller = controller(exchange);
    let stale = controller
        .apply_graph_request(default_duplex_document(3, None).into())
        .expect_err("same revision should be stale");
    assert_eq!(stale.code, ErrorCode::GraphRevisionStale);
    assert_eq!(stale.parameters["proposedRevision"], 3);

    let invalid = controller
        .apply_graph_request(
            GraphDocument::new(
                4,
                vec![NodeSpec {
                    control: mixrelay_audio_graph::NodeControl::default(),
                    id: NodeId::from("invalid-gain"),
                    label: None,
                    config: NodeConfig::Gain(GainConfig {
                        channels: 0,
                        gain_db: 0.0,
                    }),
                }],
                vec![],
            )
            .into(),
        )
        .expect_err("invalid graph should fail");
    assert_eq!(invalid.code, ErrorCode::GraphInvalid);
    assert!(invalid.parameters["issues"].is_array());

    let unsupported = controller
        .apply_graph_request(GraphDocument::new(5, vec![], vec![]).into())
        .expect_err("graph without the HAL endpoint should fail");
    assert_eq!(unsupported.code, ErrorCode::GraphBindingUnsupported);
    assert_eq!(
        unsupported.parameters["requiredEndpointNodeId"],
        DEFAULT_ENDPOINT_NODE_ID
    );
    assert_eq!(controller.graph().revision, 3);

    let mismatched = crate::test_support::controller(
        Arc::new(ProcessorExchange::new(1)),
        "another-endpoint",
        FORMAT,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    )
    .apply_graph_request(default_duplex_document(2, None).into())
    .expect_err("a different endpoint binding should fail");
    assert_eq!(mismatched.code, ErrorCode::GraphBindingUnsupported);
    assert_eq!(
        mismatched.parameters["requiredEndpointNodeId"],
        "another-endpoint"
    );
}

#[test]
fn reports_render_preparation_and_exchange_failures() {
    let exchange = Arc::new(ProcessorExchange::new(1));
    let mut failing = controller(Arc::clone(&exchange));
    failing.factory = Arc::new(crate::test_support::DspFactory {
        meters: Arc::new(PeakMeterBank::new(64, 64)),
        render_prepare: fail_render,
    });
    let render = failing
        .apply_graph_request(default_duplex_document(2, None).into())
        .expect_err("render preparation should fail");
    assert_eq!(render.code, ErrorCode::GraphInvalid);
    assert!(render.parameters["renderError"].is_string());

    exchange
        .submit(Box::new(processor(&default_duplex_document(2, None))))
        .expect("test pending processor should queue");
    let before_prepare = failing
        .prepare_and_validate(&default_duplex_document(3, None), *failing.context.read())
        .err()
        .expect("pending activation must block preparation before its failing factory");
    assert_eq!(before_prepare.code, ErrorCode::GraphExchangeBusy);
    let busy = controller(exchange)
        .apply_graph_request(default_duplex_document(3, None).into())
        .expect_err("second pending graph should be busy");
    assert_eq!(busy.code, ErrorCode::GraphExchangeBusy);
    assert_eq!(busy.parameters["proposedRevision"], 3);
}
