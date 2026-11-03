use super::*;
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
};

fn plan(revision: u64) -> Arc<RenderPlan> {
    let execution = GraphCompiler::default()
        .compile(
            &GraphDocument::new(revision, vec![], vec![]),
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 64,
                },
                limits: CompileLimits::default(),
            },
        )
        .expect("empty graph should compile");
    Arc::new(RenderPlan::prepare(&execution).expect("empty graph should prepare"))
}

#[test]
fn unchanged_observation_does_not_republish_a_bank() {
    let exchange = LatencyExchange::new();
    let plan = plan(1);
    exchange.set_plan(Arc::clone(&plan));

    assert!(exchange.refresh().unwrap());
    assert!(!exchange.refresh().unwrap());
    assert!(exchange.has_pending());

    let mut current = BlockProcessor::new(plan).unwrap();
    assert!(exchange.activate(&mut current));
    assert!(!exchange.has_pending());
}

#[test]
fn replacement_releases_old_observations_before_any_fallible_refresh() {
    let exchange = LatencyExchange::new();
    let previous = plan(1);
    let previous_owner = Arc::downgrade(&previous);
    exchange.set_plan(previous);
    assert!(exchange.refresh().unwrap());
    assert!(exchange.has_pending());
    exchange.set_plan(plan(2));
    // This must hold even if the next refresh never runs or cannot allocate a new bank.
    assert!(!exchange.has_pending());
    assert!(exchange.state.lock().prepared.is_none());
    assert!(previous_owner.upgrade().is_none());
}

#[test]
fn future_bank_stays_pending_until_its_topology_is_active() {
    let exchange = LatencyExchange::new();
    let active_plan = plan(1);
    let future_plan = plan(2);
    exchange.set_plan(Arc::clone(&future_plan));
    assert!(exchange.refresh().unwrap());

    let mut active = BlockProcessor::new(Arc::clone(&active_plan)).unwrap();
    assert!(!exchange.activate(&mut active));
    assert!(exchange.has_pending());
    assert!(Arc::ptr_eq(active.plan(), &active_plan));

    let mut future = BlockProcessor::new(Arc::clone(&future_plan)).unwrap();
    assert!(exchange.activate(&mut future));
    assert!(!exchange.has_pending());
    assert!(Arc::ptr_eq(future.plan(), &future_plan));
}

#[test]
fn stale_or_same_revision_foreign_bank_is_retired_off_audio() {
    for stale_revision in [1, 2] {
        let exchange = LatencyExchange::new();
        let current_plan = plan(2);
        exchange.set_plan(plan(stale_revision));
        assert!(exchange.refresh().unwrap());

        let mut current = BlockProcessor::new(current_plan).unwrap();
        assert!(!exchange.activate(&mut current));
        assert!(!exchange.has_pending());
        assert!(!exchange.retired.load(Ordering::Acquire).is_null());
        exchange.reclaim();
        assert!(exchange.retired.load(Ordering::Acquire).is_null());
    }
}

#[test]
fn occupied_retirement_slot_and_audio_gate_leave_pending_bank_untouched() {
    let exchange = LatencyExchange::new();
    let plan = plan(1);
    let mut current = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    exchange.set_plan(Arc::clone(&plan));
    assert!(exchange.refresh().unwrap());
    let old_bank = Box::new(DelayBank::prepare(Arc::clone(&plan), vec![]).unwrap());
    exchange
        .retired
        .store(Box::into_raw(old_bank), Ordering::Release);
    assert!(!exchange.activate(&mut current));
    assert!(exchange.has_pending());
    exchange.reclaim();

    exchange.audio_gate.store(true, Ordering::Release);
    assert!(!exchange.activate(&mut current));
    assert!(exchange.has_pending());
    exchange.audio_gate.store(false, Ordering::Release);

    assert!(exchange.activate(&mut current));
    assert!(!exchange.has_pending());
}

#[test]
fn replaced_bank_is_retired_until_control_reclaims_it() {
    let exchange = LatencyExchange::new();
    let plan = plan(1);
    let mut current = BlockProcessor::new(Arc::clone(&plan)).unwrap();
    let previous = Box::new(DelayBank::prepare(Arc::clone(&plan), vec![]).unwrap());
    assert!(current.replace_delay_bank(previous).is_none());
    exchange.set_plan(Arc::clone(&plan));
    assert!(exchange.refresh().unwrap());

    assert!(exchange.activate(&mut current));
    assert!(!exchange.retired.load(Ordering::Acquire).is_null());
    exchange.reclaim();
    assert!(exchange.retired.load(Ordering::Acquire).is_null());
}

#[test]
fn only_acknowledged_topologies_are_available_to_the_latency_reporter() {
    let exchange = crate::ProcessorExchange::new(1);
    assert!(exchange.latency_plan().is_none());
    let first = plan(1);
    exchange.observe_plan(Arc::clone(&first));
    assert!(Arc::ptr_eq(&exchange.latency_plan().unwrap(), &first));
    let next = plan(2);
    exchange
        .submit(Box::new(BlockProcessor::new(Arc::clone(&next)).unwrap()))
        .unwrap();
    assert!(exchange.latency_plan().is_none());
    let mut current = Box::new(BlockProcessor::new(first).unwrap());
    assert!(
        exchange
            .activate_pending_configured(&mut current, |_| Ok::<(), ()>(()))
            .unwrap()
    );
    assert!(Arc::ptr_eq(&exchange.latency_plan().unwrap(), &next));
    exchange.reclaim_retired();
}
