//! Effect controls, bypass and history-transition contracts.
use super::*;

#[test]
fn effect_activation_retains_echo_and_off_invalidates_old_history_without_allocation() {
    let config = configs().remove(8);
    let mut current = processor(config.clone(), 1, FORMAT);
    let input = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let mut output = [0.0; 8];
    current
        .process_block(4, &[&input], &mut [&mut output])
        .unwrap();
    let exchange = crate::ProcessorExchange::new(1);
    exchange.submit(processor(config, 2, FORMAT)).unwrap();
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    assert!(exchange.activate_pending(&mut current));
    for _ in 0..11 {
        current
            .process_block(4, &[&[0.0; 8]], &mut [&mut output])
            .unwrap();
    }
    current
        .process_block(4, &[&[0.0; 8]], &mut [&mut output])
        .unwrap();
    assert_eq!(output[0], 1.0);
    current.effect_states[0].suspend();
    for _ in 0..30 {
        current
            .process_block(4, &[&[0.0; 8]], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [0.0; 8]);
    }
    assert_eq!(
        HEAP_OPERATIONS.with(|counter| counter.replace(None)),
        Some(0)
    );
    exchange.reclaim_retired();
}

#[test]
fn reconnecting_effect_input_discards_previous_source_tail_without_callback_allocation() {
    let config = configs().remove(8);
    let mut current = processor_from_source(config.clone(), 1, FORMAT, "source-a");
    let mut output = [0.0; 8];
    current
        .process_block(
            4,
            &[&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]],
            &mut [&mut output],
        )
        .unwrap();
    let exchange = crate::ProcessorExchange::new(1);
    exchange
        .submit(processor_from_source(config, 2, FORMAT, "source-b"))
        .unwrap();
    HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
    assert!(exchange.activate_pending(&mut current));
    for _ in 0..32 {
        current
            .process_block(4, &[&[0.0; 8]], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [0.0; 8]);
    }
    assert_eq!(
        HEAP_OPERATIONS.with(|counter| counter.replace(None)),
        Some(0)
    );
    exchange.reclaim_retired();
}

#[test]
fn every_effect_callback_and_activation_is_allocation_free_across_formats() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let format = RenderFormat {
            sample_rate: rate,
            block_frames: 128,
        };
        for config in configs() {
            let mut current = processor(config.clone(), 1, format);
            let exchange = crate::ProcessorExchange::new(1);
            exchange
                .submit(processor(config.clone(), 2, format))
                .unwrap();
            let input = vec![0.25; 128 * usize::from(config.channels)];
            let mut output = vec![0.0; 128 * usize::from(config.output_channels())];
            HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
            assert!(exchange.activate_pending(&mut current));
            for _ in 0..32 {
                current
                    .process_block(128, &[&input], &mut [&mut output])
                    .unwrap();
            }
            assert_eq!(
                HEAP_OPERATIONS.with(|counter| counter.replace(None)),
                Some(0)
            );
            assert!(output.iter().all(|v| v.is_finite()));
            exchange.reclaim_retired();
        }
    }
}

#[test]
fn parameter_edit_smooths_pan_and_bypass_without_resetting_audio_history() {
    let mut config = configs().remove(6);
    if let mixrelay_audio_graph::Effect::Pan(p) = &mut config.effect {
        p.position = -1.0;
    }
    let format = RenderFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let mut current = processor(config.clone(), 1, format);
    let input = [0.5; 128];
    let mut output = [0.0; 256];
    current
        .process_block(128, &[&input], &mut [&mut output])
        .unwrap();
    let before = output[254];
    if let mixrelay_audio_graph::Effect::Pan(p) = &mut config.effect {
        p.position = 1.0;
    }
    let exchange = crate::ProcessorExchange::new(1);
    exchange
        .submit(processor(config.clone(), 2, format))
        .unwrap();
    assert!(exchange.activate_pending(&mut current));
    current
        .process_block(128, &[&input], &mut [&mut output])
        .unwrap();
    assert!((output[0] - before).abs() < 0.003);
    for _ in 0..32 {
        current
            .process_block(128, &[&input], &mut [&mut output])
            .unwrap();
    }
    assert!(output[254].abs() < 1e-6);
    assert!((output[255] - 0.5).abs() < 1e-6);
    exchange.reclaim_retired();
    config.bypass = true;
    exchange.submit(processor(config, 3, format)).unwrap();
    assert!(exchange.activate_pending(&mut current));
    current
        .process_block(128, &[&input], &mut [&mut output])
        .unwrap();
    assert!(output[0] < 0.003);
    for _ in 0..40 {
        current
            .process_block(128, &[&input], &mut [&mut output])
            .unwrap();
    }
    assert_eq!(output, [0.5; 256]);
    exchange.reclaim_retired();
}

#[test]
fn mute_keeps_delay_processing_while_off_blocks_input_and_discards_history() {
    for enabled in [false, true] {
        let config = configs().remove(8);
        let mut current = processor(config, 1, FORMAT);
        let exchange = crate::ProcessorExchange::new(1);
        exchange
            .publish_controls(
                current.plan().controls(),
                2,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("fx"),
                    control: mixrelay_audio_graph::NodeControl {
                        enabled,
                        muted: true,
                    },
                    gain_db: None,
                }],
            )
            .unwrap();
        let mut output = [0.0; 8];
        current
            .process_block(
                4,
                &[&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]],
                &mut [&mut output],
            )
            .unwrap();
        assert_eq!(output, [0.0; 8]);
        exchange
            .publish_controls(
                current.plan().controls(),
                3,
                &[crate::RuntimeControlUpdate {
                    node_id: NodeId::from("fx"),
                    control: mixrelay_audio_graph::NodeControl::default(),
                    gain_db: None,
                }],
            )
            .unwrap();
        HEAP_OPERATIONS.with(|counter| counter.set(Some(0)));
        for _ in 0..11 {
            current
                .process_block(4, &[&[0.0; 8]], &mut [&mut output])
                .unwrap();
        }
        current
            .process_block(4, &[&[0.0; 8]], &mut [&mut output])
            .unwrap();
        assert_eq!(output[0], if enabled { 1.0 } else { 0.0 });
        assert_eq!(
            HEAP_OPERATIONS.with(|counter| counter.replace(None)),
            Some(0)
        );
    }
}
