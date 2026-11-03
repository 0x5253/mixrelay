use super::super::{Arc, BlockProcessor, RenderFormat};
use super::*;
use crate::ProcessorExchange;
use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler, GraphDocument};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counter {
    identity: HostedProcessorIdentity,
    cursor: usize,
    placeholder: bool,
    drops: Arc<AtomicUsize>,
}

impl HostedAudioProcessor for Counter {
    fn process(&mut self, _input: &[f32], output: &mut [f32], _frames: usize) -> bool {
        self.cursor += 1;
        output[0] = u16::try_from(self.cursor).map(f32::from).unwrap();
        !self.placeholder
    }
    fn reuse_identity(&self) -> Option<&HostedProcessorIdentity> {
        Some(&self.identity)
    }
    fn requests_reuse(&self) -> bool {
        self.placeholder
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        if !self.placeholder {
            self.drops.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn processor(
    revision: u64,
    placeholder: bool,
    drops: &Arc<AtomicUsize>,
    identity: &HostedProcessorIdentity,
) -> Box<BlockProcessor> {
    processor_with_identity(revision, placeholder, drops, identity.clone())
}

fn processor_with_identity(
    revision: u64,
    placeholder: bool,
    drops: &Arc<AtomicUsize>,
    identity: HostedProcessorIdentity,
) -> Box<BlockProcessor> {
    let execution = GraphCompiler::default()
        .compile(
            &GraphDocument::new(revision, vec![], vec![]),
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48000,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    let plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let processor = BlockProcessor::with_hosted(
        plan,
        vec![Box::new(Counter {
            identity,
            cursor: 0,
            placeholder,
            drops: Arc::clone(drops),
        }) as Box<dyn HostedAudioProcessor>]
        .into_boxed_slice(),
    )
    .unwrap();
    Box::new(processor)
}

#[test]
fn activation_moves_the_complete_instance_without_resetting_pcm_cursor() {
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let mut current = processor(1, false, &drops, &identity);
    let exchange = ProcessorExchange::new(1);
    let mut output = [0.0];
    assert!(current.hosted.processors[0].process(&[], &mut output, 1));
    exchange
        .submit(processor(2, true, &drops, &identity))
        .unwrap();
    assert!(
        exchange
            .activate_pending_configured(&mut current, |_| Ok::<_, ()>(()))
            .unwrap()
    );
    exchange.reclaim_retired();
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert!(current.hosted.processors[0].process(&[], &mut output, 1));
    assert_eq!(output[0].to_bits(), 2.0_f32.to_bits());
    drop(current);
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}

#[test]
fn rejected_configuration_and_cancel_do_not_take_the_active_instance() {
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let mut current = processor(1, false, &drops, &identity);
    let exchange = ProcessorExchange::new(1);
    exchange
        .submit(processor(2, true, &drops, &identity))
        .unwrap();
    assert_eq!(
        exchange.activate_pending_configured(&mut current, |_| Err("rejected")),
        Err("rejected")
    );
    assert!(!current.hosted.processors[0].requests_reuse());
    drop(exchange);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    assert!(current.hosted.processors[0].process(&[], &mut [0.0], 1));
}

#[test]
fn missing_identity_never_activates_a_placeholder() {
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let mut current = processor_with_identity(1, false, &drops, HostedProcessorIdentity::new());
    let exchange = ProcessorExchange::new(1);
    exchange
        .submit(processor(2, true, &drops, &identity))
        .unwrap();
    assert!(
        !exchange
            .activate_pending_configured(&mut current, |_| -> Result<(), ()> {
                panic!("native activation must not run without the required owner")
            })
            .unwrap()
    );
    assert_eq!(exchange.revisions().active, 1);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
}

#[test]
fn removal_marks_the_old_owner_inactive_before_retired_reclamation() {
    struct OwnerFlag(Arc<std::sync::atomic::AtomicBool>);
    impl HostedAudioProcessor for OwnerFlag {
        fn process(&mut self, _: &[f32], _: &mut [f32], _: usize) -> bool {
            true
        }
        fn set_active(&mut self, active: bool) {
            self.0.store(active, Ordering::Release);
        }
    }
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let mut current = processor(1, false, &drops, &identity);
    current.hosted = HostedRack::new(
        &current.plan,
        vec![Box::new(OwnerFlag(Arc::clone(&active))) as Box<dyn HostedAudioProcessor>]
            .into_boxed_slice(),
    )
    .unwrap();
    let mut removed = processor(2, true, &drops, &identity);
    removed.hosted = HostedRack::new(&removed.plan, Box::new([])).unwrap();
    let exchange = ProcessorExchange::new(1);
    exchange.submit(removed).unwrap();
    assert!(
        exchange
            .activate_pending_configured(&mut current, |_| Ok::<_, ()>(()))
            .unwrap()
    );
    // The retired holder still owns the flag; it is no longer a reuse candidate.
    assert_eq!(Arc::strong_count(&active), 2);
    assert!(!active.load(Ordering::Acquire));
    exchange.reclaim_retired();
    assert_eq!(Arc::strong_count(&active), 1);
}

#[test]
fn pending_and_retired_shutdown_release_each_hosted_owner_once() {
    for _ in 0..128 {
        let drops = Arc::new(AtomicUsize::new(0));
        let identity = HostedProcessorIdentity::new();
        let pending = ProcessorExchange::new(1);
        pending
            .submit(processor(2, false, &drops, &identity))
            .unwrap();
        drop(pending);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        assert_eq!(Arc::strong_count(&drops), 1);

        let exchange = ProcessorExchange::new(1);
        let mut current = processor(1, false, &drops, &identity);
        exchange
            .submit(processor(2, false, &drops, &identity))
            .unwrap();
        assert!(exchange.activate_pending(&mut current));
        assert_eq!(drops.load(Ordering::Relaxed), 1);
        drop(exchange);
        assert_eq!(drops.load(Ordering::Relaxed), 2);
        assert_eq!(Arc::strong_count(&drops), 2);
        drop(current);
        assert_eq!(drops.load(Ordering::Relaxed), 3);
        assert_eq!(Arc::strong_count(&drops), 1);
    }
}

#[test]
fn reuse_replacement_and_removal_never_accumulate_hosted_owners() {
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let exchange = ProcessorExchange::new(1);
    let mut current = processor(1, false, &drops, &identity);
    for revision in 2..=1_025 {
        let reuse = revision % 2 == 0;
        let prepared_identity = if reuse {
            current.hosted.processors[0]
                .reuse_identity()
                .unwrap()
                .clone()
        } else {
            HostedProcessorIdentity::new()
        };
        exchange
            .submit(processor(revision, reuse, &drops, &prepared_identity))
            .unwrap();
        assert!(exchange.activate_pending(&mut current));
        exchange.reclaim_retired();
        // This includes placeholders: their retained Arc must also be released.
        assert_eq!(Arc::strong_count(&drops), 2);
        assert_eq!(
            drops.load(Ordering::Relaxed),
            usize::try_from((revision - 1) / 2).unwrap()
        );
    }
    let mut removed = processor(1_026, true, &drops, &identity);
    removed.hosted = HostedRack::new(&removed.plan, Box::new([])).unwrap();
    exchange.submit(removed).unwrap();
    assert!(exchange.activate_pending(&mut current));
    exchange.reclaim_retired();
    assert_eq!(drops.load(Ordering::Relaxed), 513);
    assert_eq!(Arc::strong_count(&drops), 1);
}

#[test]
fn failed_submission_and_activation_release_hosted_owners_without_touching_active() {
    let drops = Arc::new(AtomicUsize::new(0));
    let identity = HostedProcessorIdentity::new();
    let exchange = ProcessorExchange::new(1);
    let mut current = processor(1, false, &drops, &identity);
    assert_eq!(
        exchange.submit_configured(processor(2, false, &drops, &identity), || Err(
            "configuration failed"
        )),
        Err(crate::ProcessorConfigurationError::Configuration(
            "configuration failed"
        ))
    );
    assert_eq!(drops.load(Ordering::Relaxed), 1);
    exchange
        .submit(processor(2, false, &drops, &identity))
        .unwrap();
    assert_eq!(
        exchange.submit(processor(3, false, &drops, &identity)),
        Err(crate::ProcessorSubmitError::Busy {
            proposed_revision: 3
        })
    );
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    assert_eq!(
        exchange.activate_pending_configured(&mut current, |_| Err("activation failed")),
        Err("activation failed")
    );
    assert_eq!(drops.load(Ordering::Relaxed), 2);
    drop(exchange);
    assert_eq!(drops.load(Ordering::Relaxed), 3);
    let mut output = [0.0];
    assert!(current.hosted.processors[0].process(&[], &mut output, 1));
    assert_eq!(output[0].to_bits(), 1.0_f32.to_bits());
    drop(current);
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    assert_eq!(Arc::strong_count(&drops), 1);
}
