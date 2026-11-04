//! Reservation failures are injected locally; these tests never exhaust host memory.
use super::*;
use crate::ProcessorExchange;
use mixrelay_audio_graph::{CompressorConfig, Effect};

fn storage_plan() -> Arc<RenderPlan> {
    let reverb = builtin_effects::configs()
        .into_iter()
        .find(|config| matches!(config.effect, Effect::Reverb(_)))
        .unwrap();
    let execution = compile(
        vec![
            device_input("source"),
            node("reverb", NodeConfig::Effect(reverb)),
            node(
                "comp",
                NodeConfig::Compressor(CompressorConfig {
                    version: 1,
                    bypass: false,
                    channels: 2,
                    threshold_db: -18.0,
                    ratio: 4.0,
                    attack_ms: 10.0,
                    release_ms: 120.0,
                    makeup_gain_db: 0.0,
                }),
            ),
            node("bus", NodeConfig::DeviceBus { channels: 2 }),
            device_output("sink"),
        ],
        vec![
            edge("a", "source", AUDIO_PORT_ID, "reverb", AUDIO_PORT_ID),
            edge("b", "reverb", AUDIO_PORT_ID, "comp", AUDIO_PORT_ID),
            edge("c", "comp", AUDIO_PORT_ID, "bus", AUDIO_PORT_ID),
            edge("d", "bus", AUDIO_PORT_ID, "sink", AUDIO_PORT_ID),
        ],
    );
    let mut plan = RenderPlan::prepare(&execution).unwrap();
    // Exercise the shared scratch reservation without starting a native plug-in.
    plan.hosted_scratch_samples = 8;
    Arc::new(plan)
}

#[test]
fn every_pcm_reservation_failure_returns_and_reclaims_partial_storage() {
    let plan = storage_plan();
    // Bound the search above this fixture's effect lines, history, arena and scratch reservations.
    for successful_reservations in 0..64 {
        HEAP_BALANCE.with(|counter| counter.set(Some(HeapBalance::default())));
        let result = buffers::fault_injection::fail_after(successful_reservations, || {
            BlockProcessor::new(Arc::clone(&plan))
        });
        let prepared = match result {
            Ok(processor) => Some(processor),
            Err(error) => {
                assert!(matches!(error, RenderPlanError::BufferPreparation(_)));
                None
            }
        };
        let complete = prepared.is_some();
        if let Some(processor) = &prepared {
            assert_eq!(processor.hosted.scratch_samples(), 8);
            assert!(
                processor
                    .dynamics_minimum_gains
                    .iter()
                    .all(|&gain| gain.to_bits() == 1.0_f32.to_bits())
            );
        }
        drop(prepared);
        let balance = HEAP_BALANCE.with(|counter| counter.replace(None).unwrap());
        assert_eq!(balance.allocated_bytes, balance.freed_bytes, "{balance:?}");
        assert_eq!(balance.allocations, balance.frees, "{balance:?}");
        assert_eq!(Arc::strong_count(&plan), 1);
        if complete {
            // Preserve the original arena, external PCM, dynamics, bus and scratch failure cases.
            assert!(successful_reservations >= 10);
            return;
        }
    }
    panic!("prepared storage did not complete within the fixture's reservation bound");
}

#[test]
fn failed_preparation_preserves_active_revision_and_healthy_pcm() {
    let execution = compile(
        vec![device_input("source"), device_output("sink")],
        vec![edge(
            "route",
            "source",
            AUDIO_PORT_ID,
            "sink",
            AUDIO_PORT_ID,
        )],
    );
    let old_plan = Arc::new(RenderPlan::prepare(&execution).unwrap());
    let mut current = Box::new(BlockProcessor::new(old_plan).unwrap());
    let exchange = ProcessorExchange::new(1);
    let mut proposed_plan = RenderPlan::prepare(&execution).unwrap();
    proposed_plan.revision = 2;
    let failed =
        buffers::fault_injection::fail_after(2, || BlockProcessor::new(Arc::new(proposed_plan)));
    assert!(matches!(failed, Err(RenderPlanError::BufferPreparation(_))));
    assert_eq!(exchange.revisions().active, 1);
    assert_eq!(exchange.revisions().submitted, 1);
    assert!(!exchange.activate_pending(&mut current));
    let input = [0.25, -0.125, 0.5, -0.5, 0.75, -0.75, 1.0, -1.0];
    let mut output = [0.0; 8];
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    let result = current.process_block(4, &[&input], &mut [&mut output]);
    let operations = HEAP_OPERATIONS.with(|counter| counter.replace(None));
    result.unwrap();
    assert_eq!(operations, Some(0));
    assert_eq!(input.map(f32::to_bits), output.map(f32::to_bits));
}

#[test]
fn common_buffer_preparation_rejects_unaddressable_capacity_without_allocating_it() {
    assert!(matches!(
        buffers::filled(usize::MAX, 0.0),
        Err(RenderPlanError::BufferPreparation(_))
    ));
    assert!(buffers::filled(0, 1.0).unwrap().is_empty());
    let samples = buffers::filled(8, 0.0).unwrap();
    assert!(
        samples
            .iter()
            .all(|&sample| sample.to_bits() == 0.0_f32.to_bits())
    );
}
