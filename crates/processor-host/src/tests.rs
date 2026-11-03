//! Host factory, shared transport and control ownership regressions.

use super::*;
use crate::{
    catalog::selected_ports,
    control::HostedProcessorControl,
    hosted::HostedExternalProcessor,
    shared_transport::{ControlMailbox, ParentAudio},
    worker::broker::consume_parameters,
    worker_paths::WorkerPaths,
    worker_process::{WorkerMonitor, cleanup_worker_paths},
};
use mixrelay_audio_graph::{
    CompileContext, NodeConfig, NodeId, NodeResolver, NodeSpec, ProviderNodeConfig, RenderFormat,
};
use mixrelay_control_protocol::{ExternalProcessorDescriptor, ProcessorEditorState};
use mixrelay_render::{
    HostedAudioProcessor, HostedProcessorFactory, HostedProcessorPrepareRequest,
};
use std::{
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[test]
fn saved_selection_is_not_retargeted_by_equal_totals_or_catalog_order() {
    let stereo = mixrelay_audio_graph::ProviderPorts::canonical_stereo("stereo");
    let mut sidechain = stereo.clone();
    sidechain.layout_id = "stereo-sidechain".into();
    let mut key_bus = stereo.inputs[0].clone();
    key_bus.id = mixrelay_audio_graph::PortId::from("key");
    key_bus.kind = mixrelay_audio_graph::ProviderBusKind::Sidechain;
    sidechain.inputs.push(key_bus);
    let mut descriptor = ExternalProcessorDescriptor {
        type_id: "clap:fixture".into(),
        display_name: "Fixture".into(),
        vendor: String::new(),
        version: String::new(),
        format: mixrelay_control_protocol::ExternalProcessorFormat::Clap,
        layouts: vec![stereo.clone(), sidechain.clone()],
        default_layout_id: stereo.layout_id.clone(),
        has_editor: false,
        parameters: Vec::new(),
    };
    let mut config = ProviderNodeConfig {
        type_id: descriptor.type_id.as_str().into(),
        ports: Some(sidechain.clone()),
        parameters: serde_json::json!({}),
    };
    descriptor.layouts.reverse();
    assert_eq!(selected_ports(&descriptor, &config).unwrap(), sidechain);
    let mut renamed = sidechain.clone();
    renamed.inputs[1].name = "Translated key".into();
    config.ports = Some(renamed);
    assert_eq!(selected_ports(&descriptor, &config).unwrap(), sidechain);
    let mut wrong = sidechain.clone();
    wrong.inputs[1].kind = mixrelay_audio_graph::ProviderBusKind::Main;
    config.ports = Some(wrong);
    assert!(selected_ports(&descriptor, &config).is_err());
    config.ports = Some(sidechain);
    descriptor.layouts = vec![stereo.clone()];
    assert!(selected_ports(&descriptor, &config).is_err());
    config.ports = None;
    assert_eq!(selected_ports(&descriptor, &config).unwrap(), stereo);
}

fn saved_ports(inputs: u16, outputs: u16) -> mixrelay_audio_graph::ProviderPorts {
    let mut ports = mixrelay_audio_graph::ProviderPorts::canonical_stereo("fixture-layout");
    ports.inputs[0].channels = inputs;
    ports.inputs[0].channel_format = mixrelay_audio_graph::ProviderChannelFormat::Discrete;
    ports.outputs[0].channels = outputs;
    ports.outputs[0].channel_format = mixrelay_audio_graph::ProviderChannelFormat::Discrete;
    ports
}

#[test]
fn missing_plugin_uses_saved_ports_and_silence_instead_of_passthrough() {
    let config = ProviderNodeConfig {
        type_id: "vst3:not-installed".into(),
        parameters: serde_json::json!({}),
        ports: Some(saved_ports(8, 8)),
    };
    let node = NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: NodeId::from("missing"),
        label: None,
        config: NodeConfig::Provider(config.clone()),
    };
    let context = CompileContext {
        format: RenderFormat {
            sample_rate: 48_000,
            block_frames: 32,
        },
        limits: mixrelay_audio_graph::CompileLimits::default(),
    };
    let resolver = CatalogNodeResolver::new(ProcessorCatalog::default());
    let ports = resolver.resolve(&node, &context).unwrap();
    assert_eq!(ports.inputs[0].channels(), 8);
    let mut processor = ExternalProcessorFactory::new(
        ProcessorCatalog::default(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    )
    .prepare(HostedProcessorPrepareRequest {
        node_id: &node.id,
        config: &config,
        format: context.format,
        input_channels: 8,
        output_channels: 8,
    })
    .unwrap();
    let mut output = [1.0; 256];
    assert!(processor.process(&[0.75; 256], &mut output, 32));
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.0_f32.to_bits())
    );
    let mut invalid = node;
    if let NodeConfig::Provider(config) = &mut invalid.config {
        config.ports = Some(saved_ports(0, 65));
    }
    assert!(resolver.resolve(&invalid, &context).is_err());
}

#[cfg(not(debug_assertions))]
#[test]
fn measure_shared_audio_transport() {
    const ITERATIONS: u32 = 100_000;
    if std::env::var_os("MIXRELAY_PROCESSOR_TRANSPORT_BENCH").is_none() {
        return;
    }
    for channels in [2, 8, 64] {
        for frames in [32, 256] {
            let (shared_path, config_path, shared, mailbox) =
                audio_mapping(channels, channels, frames);
            let control = Arc::new(HostedProcessorControl::native(
                mailbox,
                std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            ));
            control.own_transport_file(shared_path.clone());
            let mut processor = HostedExternalProcessor {
                worker: None,
                shared: Some(shared),
                config_path,
                layout: AudioLayout {
                    inputs: channels,
                    outputs: channels,
                    block_frames: frames,
                },
                next_sequence: 1,
                dry_delay: dry_delay::DryDelay::new(channels, channels, frames).unwrap(),
                control: Some(control),
                timing: timing::TimingProbe::default(),
                registration: None,
            };
            let input = vec![0.25; channels * frames];
            let mut output = vec![0.0; channels * frames];
            assert!(processor.process(&input, &mut output, frames));
            let mut rounds = [0.0; 7];
            for elapsed in &mut rounds {
                let start = Instant::now();
                for _ in 0..ITERATIONS {
                    let shared = processor.shared.as_ref().expect("live benchmark mapping");
                    shared.acknowledge_benchmark_block();
                    assert!(processor.process(
                        std::hint::black_box(&input),
                        std::hint::black_box(&mut output),
                        frames,
                    ));
                }
                *elapsed = start.elapsed().as_secs_f64() * 1e9 / f64::from(ITERATIONS);
            }
            rounds.sort_by(f64::total_cmp);
            println!(
                "processor_transport channels={channels} frames={frames} median_ns={:.3}",
                rounds[3]
            );
        }
    }
}

fn shared_mapping(
    inputs: usize,
    outputs: usize,
    frames: usize,
) -> (PathBuf, PathBuf, ControlMailbox) {
    let paths = WorkerPaths::new_in(&std::env::temp_dir()).expect("private worker paths");
    let shared_path = paths.shared().to_path_buf();
    let config_path = paths.config().to_path_buf();
    let (audio, control) = shared_transport::create_owned(
        &paths,
        AudioLayout {
            inputs,
            outputs,
            block_frames: frames,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .expect("shared transport");
    drop(audio);
    (shared_path, config_path, control)
}

#[test]
fn worker_setup_cleanup_removes_the_owned_transport() {
    let paths = WorkerPaths::new_in(&std::env::temp_dir()).expect("private worker paths");
    let shared_path = paths.shared().to_path_buf();
    let config_path = paths.config().to_path_buf();
    let (audio, control) = shared_transport::create_owned(
        &paths,
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 128,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .expect("shared transport");
    drop(control);
    drop(audio);
    assert!(shared_path.exists());
    cleanup_worker_paths(&shared_path, &config_path);
    assert!(!shared_path.exists());
}

fn audio_mapping(
    inputs: usize,
    outputs: usize,
    frames: usize,
) -> (PathBuf, PathBuf, ParentAudio, ControlMailbox) {
    let paths = WorkerPaths::new_in(&std::env::temp_dir()).expect("private worker paths");
    let shared_path = paths.shared().to_path_buf();
    let config_path = paths.config().to_path_buf();
    let (audio, control) = shared_transport::create_owned(
        &paths,
        AudioLayout {
            inputs,
            outputs,
            block_frames: frames,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .expect("shared transport");
    (shared_path, config_path, audio, control)
}

#[test]
fn stalled_or_failed_worker_does_not_block_an_independent_processor() {
    let (mut unavailable, _) = test_processor();
    let (mut healthy, healthy_path) = test_processor();
    let (mut worker, _) = shared_transport::worker(&healthy_path).unwrap();
    let mut output = [0.0; 8];
    let input = [0.25; 8];
    assert!(unavailable.process(&input, &mut output, 4));
    assert!(healthy.process(&input, &mut output, 4));
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.0_f32.to_bits())
    );
    for sequence in 1..=64 {
        assert_eq!(worker.receive(|_| {}), Some(4));
        assert!(worker.publish(|output| output.fill(0.5)));
        if sequence == 32 {
            (unavailable.shared.as_ref().unwrap())
                .header()
                .worker_status
                .store(WORKER_FAILED, Ordering::Release);
        }
        assert!(unavailable.process(&input, &mut output, 4));
        assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
        assert!(healthy.process(&input, &mut output, 4));
        assert!(
            output
                .iter()
                .all(|sample| (*sample - 0.5).abs() < f32::EPSILON)
        );
    }
    assert_eq!(
        unavailable.next_sequence, 2,
        "a stalled mailbox cannot accumulate requests"
    );
    assert_eq!(healthy.next_sequence, 66);
}

fn test_processor() -> (HostedExternalProcessor, PathBuf) {
    let (shared_path, config_path, shared, mailbox) = audio_mapping(2, 2, 4);
    let control = Arc::new(HostedProcessorControl::native(
        mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    control.own_transport_file(shared_path.clone());
    let processor = HostedExternalProcessor {
        worker: None,
        shared: Some(shared),
        config_path,
        layout: AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        next_sequence: 1,
        dry_delay: dry_delay::DryDelay::new(2, 2, 4).unwrap(),
        control: Some(control),
        timing: timing::TimingProbe::default(),
        registration: None,
    };
    (processor, shared_path)
}

#[test]
fn startup_padding_does_not_trigger_immediate_graph_bypass_or_repeat_audio() {
    let (mut processor, shared_path) = test_processor();
    let (mut worker, _) = shared_transport::worker(&shared_path).unwrap();
    let first = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0];
    let mut output = [f32::NAN; 8];
    assert!(
        processor.process(&first, &mut output, 4),
        "startup silence is a valid delayed output, not a request for immediate graph bypass"
    );
    assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    assert_eq!(worker.receive(|source| assert_eq!(source, first)), Some(4));
    assert!(worker.publish(|destination| destination.copy_from_slice(&first)));
    let second = [5.0, 50.0, 6.0, 60.0, 7.0, 70.0, 8.0, 80.0];
    assert!(processor.process(&second, &mut output, 4));
    assert_eq!(output.map(f32::to_bits), first.map(f32::to_bits));
    assert_eq!(worker.receive(|source| assert_eq!(source, second)), Some(4));
    assert!(worker.publish(|destination| destination.copy_from_slice(&second)));
    assert!(processor.process(&[0.0; 8], &mut output, 4));
    assert_eq!(output.map(f32::to_bits), second.map(f32::to_bits));
}

#[test]
fn missed_response_preserves_dry_delay_across_uneven_callbacks_and_suspend() {
    let (mut processor, _) = test_processor();
    let mut output = [f32::NAN; 8];
    assert!(processor.process(&[1.0; 6], &mut output[..6], 3));
    assert_eq!(output[..6], [0.0; 6]);
    // The callback straddles startup padding, but there is no worker response.
    assert!(processor.process(&[2.0; 4], &mut output[..4], 2));
    assert_eq!(output[..4], [0.0, 0.0, 1.0, 1.0]);
    assert!(processor.process(&[3.0; 8], &mut output, 4));
    assert_eq!(
        output.map(f32::to_bits),
        [1.0_f32, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0].map(f32::to_bits)
    );
    let allocations = allocation_counter::measure(|| {
        for _ in 0..100 {
            processor.suspend_processing();
        }
        assert!(processor.process(&[4.0; 8], &mut output, 4));
        assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        assert!(processor.process(&[5.0; 8], &mut output, 4));
        assert_eq!(output.map(f32::to_bits), [4.0_f32; 8].map(f32::to_bits));
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn late_worker_reports_backpressure_and_never_replays_wet_pcm_into_quiet_input() {
    let (mut processor, shared_path) = test_processor();
    let (mut worker, _) = shared_transport::worker(&shared_path).unwrap();
    let mut output = [9.0; 8];
    let audible = [0.25, -0.5, 0.0, 0.75, -0.25, 0.5, 0.0, -0.75];
    let allocations = allocation_counter::measure(|| {
        assert!(processor.process(&audible, &mut output, 4));
        assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        assert_eq!(
            worker.header().process_faults.total(),
            0,
            "startup is valid silence"
        );
        assert_eq!(
            worker.receive(|source| assert_eq!(source, audible)),
            Some(4)
        );
        assert!(processor.process(&[0.0; 8], &mut output, 4));
        assert_eq!(output.map(f32::to_bits), audible.map(f32::to_bits));
        assert!(worker.publish(|wet| wet.fill(7.0)));
        for _ in 0..2 {
            assert!(processor.process(&[0.0; 8], &mut output, 4));
            assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        }
    });
    assert_eq!(allocations.count_total, 0);
    assert_eq!(worker.header().process_faults.total(), 5);
    let mut observer = crate::process_fault::FaultObserver::default();
    let deltas = observer.observe(&worker.header().process_faults);
    let counts: Vec<_> = deltas
        .into_iter()
        .flatten()
        .map(|delta| (delta.kind, delta.new_failures, delta.total))
        .collect();
    assert_eq!(
        counts,
        [
            (crate::process_fault::FaultKind::OutputDeadlineMissed, 3, 3),
            (crate::process_fault::FaultKind::InputBackpressure, 2, 2),
        ]
    );
}

#[test]
fn suspended_processor_rejects_late_worker_audio_and_resumes_fresh_packets() {
    let (mut processor, shared_path) = test_processor();
    let (mut worker, _) = shared_transport::worker(&shared_path).unwrap();
    let mut output = [f32::NAN; 8];
    assert!(processor.process(&[7.0; 8], &mut output, 4));
    assert_eq!(worker.receive(|_| {}), Some(4));
    processor.suspend_processing();
    assert!(worker.publish(|pcm| pcm.fill(7.0)));
    assert!(processor.process(&[2.0; 8], &mut output, 4));
    assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
    assert_eq!(worker.receive(|pcm| assert_eq!(pcm, [2.0; 8])), Some(4));
    assert!(worker.publish(|pcm| pcm.fill(3.0)));
    assert!(processor.process(&[4.0; 8], &mut output, 4));
    assert_eq!(output.map(f32::to_bits), [3.0_f32; 8].map(f32::to_bits));
}

#[test]
fn consumed_input_before_output_publication_does_not_stall_the_mailbox() {
    let (mut processor, shared_path) = test_processor();
    let (mut worker, _) = shared_transport::worker(&shared_path).unwrap();
    let input = [0.25; 8];
    let mut output = [0.0; 8];
    assert!(processor.process(&input, &mut output, 4));
    assert_eq!(worker.receive(|_| {}), Some(4));
    assert!(processor.process(&input, &mut output, 4));
    assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
    assert_eq!(
        processor.next_sequence, 3,
        "consumed input releases the slot before output"
    );
    assert!(worker.publish(|output| output.fill(1.0)));
    assert!(
        processor.process(&input, &mut output, 4),
        "late output must not prevent aligned dry audio"
    );
    assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
    assert_eq!(worker.receive(|_| {}), Some(4));
    assert!(worker.publish(|output| output.fill(2.0)));
    assert!(
        processor.process(&input, &mut output, 4),
        "the busy interval uses aligned dry audio"
    );
    assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
    for sequence in 3..=66 {
        assert_eq!(worker.receive(|_| {}), Some(4));
        let processed = f32::from(u16::try_from(sequence).unwrap());
        assert!(worker.publish(|output| output.fill(processed)));
        assert!(processor.process(&input, &mut output, 4));
        assert_eq!(processor.next_sequence, sequence + 2);
        assert!(
            output
                .iter()
                .all(|sample| sample.to_bits() == processed.to_bits())
        );
        assert_eq!(
            processor.shared.as_ref().unwrap().acknowledged_output(),
            sequence
        );
    }
}

#[test]
fn shared_audio_regions_round_trip_without_growing() {
    let (shared_path, config_path, mut shared, control) = audio_mapping(2, 2, 4);
    let (mut worker, _) = shared_transport::worker(&shared_path).unwrap();
    let original_length = std::fs::metadata(&shared_path).unwrap().len();
    let input = [1.0, 10.0, 2.0, 20.0, 3.0, 30.0, 4.0, 40.0];
    assert!(shared.submit(1, &input, 4, || {}));
    assert_eq!(worker.receive(|source| assert_eq!(source, input)), Some(4));
    let expected = [5.0, 50.0, 6.0, 60.0, 7.0, 70.0, 8.0, 80.0];
    assert!(worker.publish(|destination| destination.copy_from_slice(&expected)));
    let mut output = [0.0; 8];
    assert!(shared.receive(&mut output, 4, |_| {}));
    assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
    assert_eq!(
        std::fs::metadata(&shared_path).unwrap().len(),
        original_length
    );
    drop(worker);
    drop(shared);
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
    assert!(!shared_path.exists());
}

#[test]
fn timing_status_requires_an_active_healthy_owner() {
    let (_, shared, _, _) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let clock = Arc::new(mixrelay_time::ManualClock::default());
    let control = Arc::new(HostedProcessorControl::native(shared, clock.clone()));
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("timed-node");
    let registration = registry.register(&node, &control);
    let mut probe = timing::TimingProbe::default();
    control.timing.request();
    probe.submitted(1, &control.timing, clock.as_ref());
    clock.advance(Duration::from_millis(1));
    probe.received(1, &control.timing, clock.as_ref());
    assert_eq!(registry.processing_time_ns(&node), None);
    control.active_owner.store(true, Ordering::Release);
    let header = control.shared.header();
    header.worker_status.store(WORKER_READY, Ordering::Release);
    assert!(registry.processing_time_ns(&node).is_some());
    header.worker_status.store(WORKER_FAILED, Ordering::Release);
    assert_eq!(registry.processing_time_ns(&node), None);
    drop(registration);
    drop(control);
    assert_eq!(registry.processing_time_ns(&node), None);
}

#[test]
fn retry_identity_tracks_only_the_active_failed_owner_and_is_rate_limited_per_instance() {
    let (_, failed_mailbox, _, _) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    );
    let failed = Arc::new(HostedProcessorControl::native(
        failed_mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    failed.active_owner.store(true, Ordering::Release);
    failed
        .shared
        .header()
        .worker_status
        .store(WORKER_FAILED, Ordering::Release);
    let failed_id = failed.shared.header().instance_id;
    let registry = ProcessorControlRegistry::default();
    let node = NodeId::from("retry-node");
    let failed_registration = registry.register(&node, &failed);

    assert_eq!(registry.failed_worker_instance(&node), Some(failed_id));
    registry
        .begin_failed_worker_retry(&node, failed_id, Duration::from_mins(1))
        .expect("first explicit retry should be admitted");
    assert_eq!(
        registry.begin_failed_worker_retry(&node, failed_id, Duration::from_mins(1)),
        Err("processor retry is cooling down")
    );

    failed.active_owner.store(false, Ordering::Release);
    let (_, healthy_mailbox, _, _) = shared_transport::memory::endpoints(
        AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        },
        crate::instance_id::InstanceId::new(2).unwrap(),
    );
    let healthy = Arc::new(HostedProcessorControl::native(
        healthy_mailbox,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    healthy.active_owner.store(true, Ordering::Release);
    healthy
        .shared
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let healthy_id = healthy.shared.header().instance_id;
    let healthy_registration = registry.register(&node, &healthy);
    assert_ne!(healthy_id, failed_id);
    assert_eq!(registry.failed_worker_instance(&node), None);
    assert_eq!(
        registry.begin_failed_worker_retry(&node, failed_id, Duration::ZERO),
        Err("processor instance changed")
    );

    healthy
        .shared
        .header()
        .worker_status
        .store(WORKER_FAILED, Ordering::Release);
    assert_eq!(registry.failed_worker_instance(&node), Some(healthy_id));
    registry
        .begin_failed_worker_retry(&node, healthy_id, Duration::ZERO)
        .expect("a replacement crash gets its own retry budget");

    drop(healthy_registration);
    drop(failed_registration);
    drop(healthy);
    drop(failed);
}

#[test]
fn control_mailboxes_coalesce_and_registration_does_not_extend_lifetime() {
    let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
    let control = Arc::new(HostedProcessorControl::native(
        shared,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let registry = ProcessorControlRegistry::default();
    let node_id = NodeId::from("vst-node");
    let registration = registry.register(&node_id, &control);
    registry
        .set_parameter(&node_id, 7, 0.25)
        .expect("first parameter should publish");
    registry
        .set_parameter(&node_id, 9, 0.75)
        .expect("different parameter must retain the earlier ID");
    registry.set_parameter(&node_id, 7, 0.5).unwrap();
    let header = control.shared.header();
    let mut received = Vec::new();
    consume_parameters(header, |id, value| received.push((id, value)));
    assert_eq!(received, vec![(7, 0.5), (9, 0.75)]);
    consume_parameters(header, |_, _| panic!("consumed changes must not repeat"));

    header.worker_status.store(WORKER_READY, Ordering::Release);
    let opening = registry
        .set_editor_open(&node_id, true)
        .expect("editor request should publish");
    assert_eq!(opening.state, ProcessorEditorState::Opening);
    assert_eq!(header.editor_sequence.load(Ordering::Acquire), 1);
    assert_eq!(
        header.editor_command.load(Ordering::Relaxed),
        EDITOR_COMMAND_OPEN
    );

    header.editor_width.store(800, Ordering::Relaxed);
    header.editor_height.store(600, Ordering::Relaxed);
    header.editor_resizable.store(1, Ordering::Relaxed);
    header
        .editor_state
        .store(EDITOR_STATE_OPEN, Ordering::Release);
    let opened = registry
        .editor_status(&node_id)
        .expect("live editor status should be readable");
    assert_eq!(opened.state, ProcessorEditorState::Open);
    assert_eq!((opened.width, opened.height), (Some(800), Some(600)));
    assert!(opened.resizable);

    registry
        .set_editor_open(&node_id, false)
        .expect("close request should publish");
    assert_eq!(header.editor_sequence.load(Ordering::Acquire), 2);
    assert_eq!(
        header.editor_command.load(Ordering::Relaxed),
        EDITOR_COMMAND_CLOSE
    );

    drop(registration);
    assert!(registry.controls.read().unwrap().is_empty());
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
    assert!(!shared_path.exists());
}

#[test]
fn parameter_mailbox_rejects_capacity_without_losing_other_ids() {
    let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
    let control = HostedProcessorControl::native(
        shared,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    );
    for id in 0..u32::try_from(PARAMETER_CAPACITY).unwrap() {
        control.set_parameter(id, f64::from(id) / 4096.0).unwrap();
    }
    assert!(control.set_parameter(u32::MAX, 0.5).is_err());
    control.set_parameter(0, 0.75).unwrap();
    let mut count = 0;
    consume_parameters(control.shared.header(), |id, value| {
        let expected = if id == 0 {
            0.75
        } else {
            f64::from(id) / 4096.0
        };
        assert!((value - expected).abs() < f64::EPSILON);
        count += 1;
    });
    assert_eq!(count, PARAMETER_CAPACITY);
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
}

#[test]
fn state_snapshot_roundtrips_binary_and_rejects_failed_or_oversized_exports() {
    for (state, expected_error) in [
        (Ok(vec![0, 1, 255, 13, 0]), None),
        (
            Err(ProcessorStateError::NativeFailure),
            Some(ProcessorStateError::NativeFailure),
        ),
        (
            Ok(vec![0; STATE_CAPACITY + 1]),
            Some(ProcessorStateError::TooLarge),
        ),
        (
            Err(ProcessorStateError::Unsupported),
            Some(ProcessorStateError::Unsupported),
        ),
    ] {
        let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
        let control = Arc::new(HostedProcessorControl::native(
            shared,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        control
            .shared
            .header()
            .worker_status
            .store(WORKER_READY, Ordering::Release);
        let (_, worker_control) = shared_transport::worker(&shared_path).unwrap();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while worker_control.pending_request().is_none() {
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
            worker_control.publish(1, state);
        });
        let result = control.snapshot_state(Duration::from_secs(2));
        worker.join().unwrap();
        if let Some(message) = expected_error {
            assert_eq!(result.unwrap_err(), message);
        } else {
            let saved = result.unwrap();
            assert_eq!(saved, vec![0, 1, 255, 13, 0]);
        }
        drop(control);
        cleanup_worker_paths(&shared_path, &config_path);
    }
}

#[test]
fn concurrent_parameter_edits_never_pair_a_value_with_another_id() {
    let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
    let control = Arc::new(HostedProcessorControl::native(
        shared,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let finished = Arc::new(AtomicBool::new(false));
    let producer_control = Arc::clone(&control);
    let producer_finished = Arc::clone(&finished);
    let producer = thread::spawn(move || {
        for _ in 0..1000 {
            for id in 0..64 {
                producer_control
                    .set_parameter(id, f64::from(id) / 64.0)
                    .unwrap();
            }
        }
        producer_finished.store(true, Ordering::Release);
    });
    let mut seen = [false; 64];
    let mut consume = || {
        consume_parameters(control.shared.header(), |id, value| {
            assert!((value - f64::from(id) / 64.0).abs() < f64::EPSILON);
            seen[id as usize] = true;
        });
    };
    while !finished.load(Ordering::Acquire) {
        consume();
    }
    producer.join().unwrap();
    consume();
    assert!(seen.into_iter().all(|seen| seen));
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
}

#[test]
fn timed_out_state_request_is_not_reused_until_worker_acknowledges() {
    let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
    let control = HostedProcessorControl::native(
        shared,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    );
    let (_, worker) = shared_transport::worker(&shared_path).unwrap();
    let header = control.shared.header();
    header.worker_status.store(WORKER_READY, Ordering::Release);
    assert!(
        control
            .snapshot_state(Duration::ZERO)
            .unwrap_err()
            .eq(&ProcessorStateError::Timeout)
    );
    assert!(
        control
            .snapshot_state(Duration::ZERO)
            .unwrap_err()
            .eq(&ProcessorStateError::Pending)
    );
    assert_eq!(worker.pending_request().unwrap().0, 1);
    worker.publish(1, Ok(vec![8]));
    assert!(
        control
            .snapshot_state(Duration::ZERO)
            .unwrap_err()
            .eq(&ProcessorStateError::Timeout)
    );
    assert_eq!(worker.pending_request().unwrap().0, 2);
    header.worker_status.store(WORKER_FAILED, Ordering::Release);
    assert!(
        control
            .snapshot_state(Duration::ZERO)
            .unwrap_err()
            .eq(&ProcessorStateError::Unavailable)
    );
    drop(worker);
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
}

#[cfg(unix)]
#[test]
fn monitor_reaps_a_crashed_worker_and_marks_transport_failed() {
    let (shared_path, config_path, shared) = shared_mapping(2, 2, 4);
    let control = Arc::new(HostedProcessorControl::native(
        shared,
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let mut command = Command::new("/usr/bin/false");
    let child = command.spawn().expect("fixture process should spawn");
    let mut monitor = WorkerMonitor::spawn(
        child,
        Arc::clone(&control),
        crate::worker_process::Timeouts::default(),
    )
    .expect("monitor should spawn");
    let deadline = Instant::now() + Duration::from_secs(2);
    while control
        .shared
        .header()
        .worker_status
        .load(Ordering::Acquire)
        != WORKER_FAILED
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        control
            .shared
            .header()
            .worker_status
            .load(Ordering::Acquire),
        WORKER_FAILED
    );
    monitor.stop_and_join();
    drop(control);
    cleanup_worker_paths(&shared_path, &config_path);
}
