use super::*;
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, ExecutionPlan, GraphCompiler, GraphDocument, RenderFormat,
};

fn plan(revision: u64) -> Arc<ExecutionPlan> {
    Arc::new(
        GraphCompiler::default()
            .compile(
                &GraphDocument::new(revision, vec![], vec![]),
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 192_000,
                        block_frames: 64,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .expect("empty graph should compile"),
    )
}

fn processor(revision: u64) -> BlockProcessor {
    let render = RenderPlan::prepare(&plan(revision)).expect("empty plan should lower");
    BlockProcessor::new(Arc::new(render)).unwrap()
}

#[test]
fn exchanges_prepared_processors_without_blocking_the_audio_boundary() {
    let exchange = ProcessorExchange::new(1);
    let mut current = Box::new(processor(1));

    assert!(!exchange.activate_pending(&mut current));
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 1,
            submitted: 1,
        }
    );
    exchange
        .submit(Box::new(processor(2)))
        .expect("revision should queue");
    assert_eq!(
        exchange.submit(Box::new(processor(3))),
        Err(ProcessorSubmitError::Busy {
            proposed_revision: 3,
        })
    );
    assert!(exchange.activate_pending(&mut current));
    assert_eq!(current.plan().revision(), 2);
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 2,
            submitted: 2,
        }
    );

    exchange
        .submit(Box::new(processor(3)))
        .expect("submitting again should reclaim the retired processor");
    assert!(exchange.activate_pending(&mut current));
    assert_eq!(current.plan().revision(), 3);
}

#[test]
fn exchange_gate_never_makes_the_audio_thread_wait() {
    let exchange = ProcessorExchange::new(1);
    let mut current = Box::new(processor(1));
    exchange.gate.store(true, Ordering::Release);

    assert_eq!(
        exchange.submit(Box::new(processor(2))),
        Err(ProcessorSubmitError::Busy {
            proposed_revision: 2,
        })
    );
    assert!(!exchange.activate_pending(&mut current));
    exchange.gate.store(false, Ordering::Release);
}

#[test]
fn idle_audio_polling_does_not_reject_a_first_publication() {
    use std::sync::mpsc::sync_channel;

    // A fresh exchange per trial excludes real pending/activation conflicts.
    // Both processors are prepared before racing the native-style idle poll
    // against the one permitted control writer. Never retry a rejected plan.
    let (work_tx, work_rx) =
        sync_channel::<(Arc<ProcessorExchange>, Box<BlockProcessor>, Arc<AtomicBool>)>(0);
    let (ready_tx, ready_rx) = sync_channel(0);
    let (drained_tx, drained_rx) = sync_channel(0);
    let audio = std::thread::spawn(move || {
        for (exchange, mut current, stop) in work_rx {
            ready_tx.send(()).unwrap();
            while !stop.load(Ordering::Acquire) {
                exchange.activate_pending(&mut current);
            }
            drained_tx.send(()).unwrap();
        }
    });
    let mut rejected = 0;
    for _ in 0..4096 {
        let exchange = Arc::new(ProcessorExchange::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let current = Box::new(processor(1));
        let next = Box::new(processor(2));
        work_tx
            .send((exchange.clone(), current, stop.clone()))
            .unwrap();
        ready_rx.recv().unwrap();
        let result = exchange.submit(next);
        stop.store(true, Ordering::Release);
        drained_rx.recv().unwrap();
        match result {
            Ok(()) => {}
            Err(ProcessorSubmitError::Busy { .. }) => rejected += 1,
            Err(error) => panic!("unexpected first-publication error: {error}"),
        }
    }
    drop(work_tx);
    audio.join().unwrap();
    assert_eq!(rejected, 0, "idle polling rejected {rejected}/4096 plans");
}

#[test]
fn dropping_an_exchange_reclaims_pending_and_retired_processors() {
    let pending = ProcessorExchange::new(1);
    pending
        .submit(Box::new(processor(2)))
        .expect("processor should queue");
    drop(pending);

    let retired = ProcessorExchange::new(1);
    let mut current = Box::new(processor(1));
    retired
        .submit(Box::new(processor(2)))
        .expect("processor should queue");
    assert!(retired.activate_pending(&mut current));
    drop(retired);
}

#[test]
fn control_revisions_follow_block_ack_without_changing_topology_or_reviving_retired_banks() {
    let exchange = ProcessorExchange::new(1);
    let mut current = Box::new(processor(1));
    let old_bank = Arc::clone(current.plan().controls());
    exchange.publish_controls(&old_bank, 2, &[]).unwrap();
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 1,
            submitted: 2
        }
    );
    current.process_block(1, &[], &mut []).unwrap();
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 2,
            submitted: 2
        }
    );
    assert_eq!(
        exchange.plan_revisions(),
        ProcessorRevisions {
            active: 1,
            submitted: 1
        }
    );
    let next = Box::new(processor(3));
    let next_bank = Arc::clone(next.plan().controls());
    exchange.submit(next).unwrap();
    assert_eq!(
        exchange.publish_controls(&old_bank, 4, &[]),
        Err(RuntimeControlError::StalePlan {
            current: 3,
            proposed: 1
        })
    );
    exchange.publish_controls(&next_bank, 4, &[]).unwrap();
    current.process_block(1, &[], &mut []).unwrap();
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 2,
            submitted: 4
        }
    );
    assert!(exchange.activate_pending(&mut current));
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 3,
            submitted: 4
        }
    );
    current.process_block(1, &[], &mut []).unwrap();
    assert_eq!(
        exchange.revisions(),
        ProcessorRevisions {
            active: 4,
            submitted: 4
        }
    );
    assert_eq!(
        exchange.plan_revisions(),
        ProcessorRevisions {
            active: 3,
            submitted: 3
        }
    );
}

#[test]
fn audio_activation_does_not_lock_the_control_plane_writer() {
    let exchange = ProcessorExchange::new(1);
    let mut current = Box::new(processor(1));
    exchange.submit(Box::new(processor(2))).unwrap();
    let _writer = exchange.control_writer.lock();
    assert!(exchange.activate_pending(&mut current));
}

#[test]
fn rejected_bank_owner_and_stale_topology_never_run_configuration_side_effects() {
    let first = ProcessorExchange::new(1);
    let next = processor(2);
    let shared_plan = Arc::clone(next.plan());
    first.submit(Box::new(next)).unwrap();
    let second = ProcessorExchange::new(1);
    assert_eq!(
        second.submit_configured(
            Box::new(BlockProcessor::new(shared_plan).unwrap()),
            || -> Result<(), ()> {
                panic!("wrong-owner configuration must never run");
            }
        ),
        Err(ProcessorConfigurationError::WrongExchange {
            proposed_revision: 2
        })
    );

    let bank = processor(1);
    let exchange = ProcessorExchange::new(1);
    exchange
        .publish_controls(bank.plan().controls(), 5, &[])
        .unwrap();
    assert_eq!(
        exchange.submit_configured(Box::new(processor(2)), || -> Result<(), ()> {
            panic!("stale topology configuration must never run");
        }),
        Err(ProcessorConfigurationError::StaleRevision {
            current: 5,
            proposed: 2
        })
    );
}
