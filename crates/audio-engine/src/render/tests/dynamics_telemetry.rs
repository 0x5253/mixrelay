#![allow(clippy::float_cmp)] // Exact telemetry reset and dry identity contracts.
use super::*;
use mixrelay_audio_graph::NodeControl;

fn config(kind: &str, bypass: bool) -> NodeConfig {
    serde_json::from_value(serde_json::json!({
        "type": kind,
        "config": if kind == "compressor" {
            serde_json::json!({"version":1,"channels":2,"bypass":bypass,"thresholdDb":-18,"ratio":4,"attackMs":0.1,"releaseMs":100,"makeupGainDb":12})
        } else {
            serde_json::json!({"version":1,"channels":2,"bypass":bypass,"ceilingDb":-6,"releaseMs":100})
        },
    })).unwrap()
}

fn prepared(
    config: NodeConfig,
    revision: u64,
    meters: &Arc<crate::PeakMeterBank>,
) -> BlockProcessor {
    let graph = GraphDocument::new(
        revision,
        vec![
            device_input("in"),
            node("dynamics", config),
            device_output("out"),
        ],
        vec![
            edge("input", "in", AUDIO_PORT_ID, "dynamics", AUDIO_PORT_ID),
            edge("output", "dynamics", AUDIO_PORT_ID, "out", AUDIO_PORT_ID),
        ],
    );
    let execution = GraphCompiler::default()
        .compile(
            &graph,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48000,
                    block_frames: 256,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    BlockProcessor::with_peak_meters(
        Arc::new(RenderPlan::prepare(&execution).unwrap()),
        meters.clone(),
    )
    .unwrap()
}

fn index(processor: &BlockProcessor) -> usize {
    processor
        .plan()
        .meter_bindings()
        .iter()
        .position(|binding| binding.node_id().as_str() == "dynamics")
        .unwrap()
}

#[test]
fn compressor_meter_reads_actual_gain_before_makeup_and_linked_stereo_input_output() {
    let meters = Arc::new(crate::PeakMeterBank::new(8, 2));
    let mut processor = prepared(config("compressor", false), 1, &meters);
    let binding = index(&processor);
    assert!(processor.plan().meter_bindings()[binding].has_dynamics());
    assert!(meters.take_dynamics(1, binding, 2).is_none());
    let mut input = [0.0; 512];
    for frame in input.chunks_exact_mut(2) {
        frame.copy_from_slice(&[1.0, 0.5]);
    }
    let mut output = [0.0; 512];
    for _ in 0..8 {
        processor
            .process_block(256, &[&input], &mut [&mut output])
            .unwrap();
    }
    let reading = meters.take_dynamics(1, binding, 2).unwrap();
    assert_eq!(reading.input_peaks, [1.0, 0.5]);
    assert_eq!(reading.threshold_db, -18.0);
    assert!((reading.gain_reduction_db - 13.5).abs() < 0.001);
    assert!(meters.take_peaks(1, binding, 2)[0] > 1.0);
    // Twelve dB makeup hides most attenuation in output peaks, but GR still reads 13.5 dB.
    assert!((output[510] - 10.0_f32.powf(-1.5 / 20.0)).abs() < 0.001);
    assert!((output[511] / output[510] - 0.5).abs() < 0.001);
    let drained = meters.take_dynamics(1, binding, 2).unwrap();
    assert_eq!(drained.input_peaks, [0.0, 0.0]);
    assert_eq!(drained.gain_reduction_db, 0.0);
    assert!(meters.take_dynamics(2, binding, 2).is_none());
}

#[test]
fn limiter_meter_tracks_actual_ceiling_and_no_compression_below_threshold() {
    let meters = Arc::new(crate::PeakMeterBank::new(8, 2));
    let mut processor = prepared(config("limiter", false), 0, &meters);
    let binding = index(&processor);
    let mut output = [0.0; 512];
    processor
        .process_block(256, &[&[0.1; 512]], &mut [&mut output])
        .unwrap();
    let quiet = meters.take_dynamics(0, binding, 2).unwrap();
    assert_eq!(quiet.gain_reduction_db, 0.0);
    assert_eq!(quiet.threshold_db, -6.0);
    processor
        .process_block(256, &[&[1.0; 512]], &mut [&mut output])
        .unwrap();
    let loud = meters.take_dynamics(0, binding, 2).unwrap();
    assert!((loud.gain_reduction_db - 6.0).abs() < 0.001);
    assert!((output[510] - 10.0_f32.powf(-6.0 / 20.0)).abs() < 0.001);
    assert!(meters.take_dynamics(0, 99, 2).is_none());
}

#[test]
fn dynamics_bypass_is_dry_unity_with_zero_gr_and_transitions_without_a_gain_jump() {
    for kind in ["compressor", "limiter"] {
        let meters = Arc::new(crate::PeakMeterBank::new(8, 2));
        let mut original = prepared(config(kind, false), 1, &meters);
        let mut output = [0.0; 512];
        for _ in 0..8 {
            original
                .process_block(256, &[&[1.0; 512]], &mut [&mut output])
                .unwrap();
        }
        let previous = output[510];
        let mut bypassed = prepared(config(kind, true), 2, &meters);
        bypassed.retain_history(&mut original);
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        bypassed
            .process_block(256, &[&[1.0; 512]], &mut [&mut output])
            .unwrap();
        assert!((output[0] - previous).abs() < 0.01);
        for _ in 0..24 {
            bypassed
                .process_block(256, &[&[1.0; 512]], &mut [&mut output])
                .unwrap();
        }
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        let binding = index(&bypassed);
        // Drain transition attenuation, then observe one entirely bypassed block.
        let _ = meters.take_dynamics(2, binding, 2);
        bypassed
            .process_block(256, &[&[1.0; 512]], &mut [&mut output])
            .unwrap();
        assert_eq!(output, [1.0; 512]);
        assert_eq!(
            meters
                .take_dynamics(2, binding, 2)
                .unwrap()
                .gain_reduction_db,
            0.0
        );
        let fresh = prepared(config(kind, true), 3, &meters);
        assert_eq!(fresh.dynamics_wet[0], 0.0);
        let mut resumed = prepared(config(kind, false), 4, &meters);
        resumed.retain_history(&mut bypassed);
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        resumed
            .process_block(256, &[&[1.0; 512]], &mut [&mut output])
            .unwrap();
        assert!((output[0] - 1.0).abs() < 0.01);
        for _ in 0..24 {
            resumed
                .process_block(256, &[&[1.0; 512]], &mut [&mut output])
                .unwrap();
        }
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        let reading = meters.take_dynamics(4, index(&resumed), 2).unwrap();
        let expected_reduction = if kind == "compressor" { 13.5 } else { 6.0 };
        assert!((reading.gain_reduction_db - expected_reduction).abs() < 0.001);
    }
}

#[test]
fn mute_retains_processing_gr_while_off_clears_detector_and_input_output() {
    for kind in ["compressor", "limiter"] {
        let meters = Arc::new(crate::PeakMeterBank::new(8, 2));
        let mut processor = prepared(config(kind, false), 1, &meters);
        let binding = index(&processor);
        let mut output = [0.0; 512];
        let exchange = crate::ProcessorExchange::new(1);
        for (revision, enabled, muted) in [(2, true, true), (3, false, false), (4, true, false)] {
            exchange
                .publish_controls(
                    processor.plan().controls(),
                    revision,
                    &[crate::RuntimeControlUpdate {
                        node_id: NodeId::from("dynamics"),
                        control: NodeControl { enabled, muted },
                        gain_db: None,
                    }],
                )
                .unwrap();
            processor
                .process_block(
                    256,
                    &[&[if revision == 4 { 0.01 } else { 1.0 }; 512]],
                    &mut [&mut output],
                )
                .unwrap();
            let reading = meters.take_dynamics(revision, binding, 2).unwrap();
            let peaks = meters.take_peaks(revision, binding, 2);
            if muted {
                assert!(reading.gain_reduction_db > 0.0);
                assert_eq!(peaks, [0.0, 0.0]);
            } else if !enabled {
                assert_eq!(reading.gain_reduction_db, 0.0);
                assert_eq!(reading.input_peaks, [0.0, 0.0]);
                assert_eq!(peaks, [0.0, 0.0]);
            } else {
                assert_eq!(reading.gain_reduction_db, 0.0);
            }
        }
    }
}

#[test]
fn dynamics_telemetry_resources_are_reclaimed_and_meter_writes_are_allocation_free() {
    drop(prepared(
        config("limiter", false),
        1,
        &Arc::new(crate::PeakMeterBank::new(8, 2)),
    ));
    HEAP_BALANCE.with(|balance| balance.set(Some(HeapBalance::default())));
    for revision in 1..9 {
        let meters = Arc::new(crate::PeakMeterBank::new(8, 2));
        let owner = Arc::downgrade(&meters);
        let mut processor = prepared(config("limiter", false), revision, &meters);
        let mut output = [0.0; 512];
        HEAP_OPERATIONS.with(|count| count.set(Some(0)));
        for _ in 0..64 {
            processor
                .process_block(256, &[&[1.0; 512]], &mut [&mut output])
                .unwrap();
        }
        assert_eq!(HEAP_OPERATIONS.with(|count| count.replace(None)), Some(0));
        drop(meters);
        drop(processor);
        assert!(owner.upgrade().is_none());
    }
    let balance = HEAP_BALANCE.with(|balance| balance.replace(None)).unwrap();
    assert!(balance.allocations > 0);
    assert_eq!(balance.allocated_bytes, balance.freed_bytes);
    assert_eq!(balance.allocations, balance.frees);
}
