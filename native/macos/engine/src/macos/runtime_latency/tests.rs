use super::{Measurements, publication_summary, publication_valid};
use crate::macos::runtime_latency::fixture::{feedback_plan, observation};
use mixrelay_driver_core::{EngineLatencyLease, RuntimeLatency};
use mixrelay_render::ProcessorExchange;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn control_edits_keep_topology_measurements_and_publish_applied_control_revision() {
    use mixrelay_render::{BlockProcessor, RuntimeControlUpdate};
    let plan = Arc::new(feedback_plan());
    let exchange = ProcessorExchange::new(plan.revision());
    let mut processor = Box::new(BlockProcessor::new(Arc::clone(&plan)).unwrap());
    exchange.observe_plan(Arc::clone(&plan));
    let mut measurements = Measurements::default();
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), 48_000)]);
    let mut value = observation();
    let mut observe = |value| {
        measurements.observe(
            &plan,
            plan.revision(),
            BTreeMap::from([("virtual".into(), value)]),
            &rates,
        )[&id]
    };
    assert_eq!(observe(value), None);
    for (revision, muted) in [(2, true), (3, false)] {
        exchange
            .publish_controls(
                plan.controls(),
                revision,
                &[RuntimeControlUpdate {
                    node_id: "gain".into(),
                    control: mixrelay_audio_graph::NodeControl {
                        muted,
                        ..Default::default()
                    },
                    gain_db: Some(-6.0 * f32::from(muted)),
                }],
            )
            .unwrap();
        processor
            .process_block(32, &[&[0.0; 64]], &mut [&mut [0.0; 64]])
            .unwrap();
        assert_eq!(exchange.revisions().active, revision);
        assert_eq!(exchange.latency_plan().unwrap().revision(), 1);
        value.native.acquisition.weight += 1;
        value.native.presentation.weight += 1;
        assert_eq!(observe(value), Some(0));
        assert!(publication_valid(&exchange, &plan, revision));
        assert!(!publication_valid(&exchange, &plan, revision - 1));
    }
    let next = feedback_plan();
    // A mismatched exchange topology is sufficient to exercise the publication fence.
    let different = ProcessorExchange::new(4);
    assert!(!publication_valid(&different, &next, 4));
}

#[test]
fn publication_summaries_log_each_change_once() {
    let lease = EngineLatencyLease {
        generation: 1,
        incarnation: "run".into(),
        process_id: 2,
    };
    let id = mixrelay_audio_io::VirtualEndpointId::new("device".into());
    let entry = |frames| RuntimeLatency {
        endpoint_id: id.clone(),
        rate: 48_000,
        engine_lease: lease.clone(),
        applied_graph_revision: 7,
        input_latency_frames: frames,
    };
    let destinations = |frames| BTreeMap::from([(id.clone(), entry(frames))]);
    assert_eq!(
        publication_summary(None, 4, &BTreeMap::new()),
        "no latency plan for active plan revision 4"
    );
    assert_eq!(
        publication_summary(Some((4, 7, true)), 4, &destinations(Some(384))),
        "plan 4, applied graph 7, valid true: [device=384]"
    );
    assert_eq!(
        publication_summary(Some((4, 7, false)), 4, &destinations(None)),
        "plan 4, applied graph 7, valid false: [device=unknown]"
    );
    let mut measurements = Measurements::default();
    let summary = || publication_summary(Some((4, 7, true)), 4, &destinations(Some(384)));
    assert!(measurements.log_publication(summary()));
    assert!(!measurements.log_publication(summary()));
    assert!(measurements.log_publication(publication_summary(None, 5, &BTreeMap::new())));
}
