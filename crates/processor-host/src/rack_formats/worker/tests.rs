//! Worker activation, main-thread service, latency publication and topology checks.

use std::{
    sync::{Arc, Mutex, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use truce_rack_core::{bus::BusLayout, editor::WindowHandle};

use super::*;
use crate::rack_formats::{
    effect_layout::select_effect_layout,
    fixture::{ChannelFixture, FixtureProcessAction, channel_layout, fixture},
};

mod pcm;

#[test]
fn catalog_preserves_more_than_64_configurations_and_selects_the_last() {
    let layouts: Vec<_> = (0..80)
        .map(|index| {
            let mut layout = BusLayout::stereo();
            layout.id = Some(format!("configuration:{index}"));
            layout
        })
        .collect();
    let mut plugin = fixture(layouts.clone());
    let catalog = layouts::verified_layouts(&mut plugin).unwrap();
    assert_eq!(catalog.len(), layouts.len());
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 80);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 80);
    let selected = catalog.last().unwrap();
    assert_eq!(
        layouts::native_layout(&layouts, selected).unwrap(),
        *layouts.last().unwrap()
    );
    let mut worker = activate(plugin, 79, 128).unwrap();
    assert_eq!(worker.input_channels(), 2);
    assert_eq!(worker.output_channels(), 2);
    worker.inputs_mut()[0].fill(0.25);
    worker.inputs_mut()[1].fill(-0.5);
    worker.process(128).unwrap();
    assert_eq!(worker.outputs()[0], vec![0.25; 128]);
    assert_eq!(worker.outputs()[1], vec![-0.5; 128]);
}

fn activate(
    plugin: ChannelFixture,
    index: usize,
    frames: usize,
) -> Result<RackWorkerPlugin, String> {
    activate_with_initial_state(
        plugin,
        index,
        frames,
        false,
        &crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap(),
    )
}

fn activate_with_initial_state(
    plugin: ChannelFixture,
    index: usize,
    frames: usize,
    editor_allowed: bool,
    initial: &crate::state::InitialState,
) -> Result<RackWorkerPlugin, String> {
    let selected = plugin
        .layouts
        .get(index)
        .ok_or("fixture layout is unavailable")
        .and_then(|layout| layouts::describe(layout).map_err(|_| "fixture layout is invalid"))?;
    RackWorkerPlugin::activate(
        Box::new(plugin),
        &selected,
        &WorkerActivation {
            editor_allowed,
            sample_rate: 48_000.0,
            max_block_size: frames,
        },
        initial,
    )
}

fn worker(channels: usize, frames: usize) -> RackWorkerPlugin {
    let layout = channel_layout(u32::try_from(channels).unwrap());
    let mut worker = activate(fixture(vec![layout]), 0, frames).unwrap();
    for (index, channel) in worker.inputs_mut().iter_mut().enumerate() {
        channel.fill(f32::from(u16::try_from(index + 1).unwrap()));
    }
    worker
}

fn latency_fixture(reported_after_process: u32) -> ChannelFixture {
    let mut plugin = fixture(vec![channel_layout(1)]);
    plugin.latency_frames = Some(0);
    plugin.latency_after_process = Some(reported_after_process);
    plugin
}

fn assert_unknown_until_known(worker: &mut RackWorkerPlugin, expected_frames: u32) {
    worker.process(1).unwrap();
    assert_eq!(worker.intrinsic_latency_frames(), None);
    worker.process(1).unwrap();
    assert_eq!(worker.intrinsic_latency_frames(), Some(expected_frames));
}

#[test]
fn parameter_mutation_keeps_latency_unknown_until_known_process_snapshot() {
    let mut plugin = latency_fixture(23);
    plugin.next_process_action = Some(FixtureProcessAction::ReportUnknownLatency);
    let mut worker = activate(plugin, 0, 1).unwrap();

    assert_eq!(worker.intrinsic_latency_frames(), Some(0));
    worker.set_parameter(7, 0.75).unwrap();
    assert_eq!(worker.intrinsic_latency_frames(), None);
    assert_unknown_until_known(&mut worker, 23);
}

#[test]
fn main_thread_service_refreshes_parameter_ids_without_metadata_on_parameter_edits() {
    let plugin = latency_fixture(64);
    let request = Arc::clone(&plugin.service_request);
    let reads = Arc::clone(&plugin.metadata_reads);
    let mut worker = activate(plugin, 0, 1).unwrap();
    assert!(!worker.has_editor());
    assert_eq!(reads.load(Ordering::Relaxed), 1);

    worker.service_main_thread().unwrap();
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    request.store(1, Ordering::Release);
    worker.service_main_thread().unwrap();
    assert_eq!(reads.load(Ordering::Relaxed), 2);
    assert_eq!(worker.intrinsic_latency_frames(), None);
    assert!(worker.set_parameter(7, 0.5).is_err());
    worker.set_parameter(8, 0.5).unwrap();
    assert_eq!(reads.load(Ordering::Relaxed), 2);
    worker.process(1).unwrap();
    assert_eq!(worker.intrinsic_latency_frames(), Some(64));
}

#[test]
fn native_state_metadata_is_refreshed_before_persisted_parameter_overlay() {
    let initial = crate::state::InitialState {
        parameters: serde_json::json!({"parameterValues": {"8": 0.25}}),
        native: Some(vec![8]),
    };
    let worker = activate_with_initial_state(latency_fixture(29), 0, 1, false, &initial).unwrap();
    assert_eq!(worker.parameter_ranges[0].id, 8);
}

#[test]
fn invalid_native_state_never_activates_processing() {
    let plugin = latency_fixture(29);
    let activations = Arc::clone(&plugin.activations);
    let initial = crate::state::InitialState {
        parameters: serde_json::json!({}),
        native: Some(vec![255]),
    };
    assert!(activate_with_initial_state(plugin, 0, 1, false, &initial).is_err());
    assert_eq!(activations.load(Ordering::Relaxed), 0);
}

#[test]
fn restored_topology_mismatch_rejects_worker_and_deactivates_native_instance() {
    let plugin = latency_fixture(29);
    let deactivations = Arc::clone(&plugin.deactivations);
    let initial = crate::state::InitialState {
        parameters: serde_json::json!({}),
        native: Some(vec![254]),
    };
    assert!(activate_with_initial_state(plugin, 0, 1, false, &initial).is_err());
    assert_eq!(deactivations.load(Ordering::Relaxed), 1);
}

#[test]
fn failed_restart_retains_main_thread_editor_cleanup_ownership() {
    let mut plugin = latency_fixture(64);
    plugin.info.has_editor = true;
    let request = Arc::clone(&plugin.service_request);
    let closes = Arc::clone(&plugin.editor_closes);
    let deactivations = Arc::clone(&plugin.deactivations);
    let initial = crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap();
    let mut worker = activate_with_initial_state(plugin, 0, 1, true, &initial).unwrap();
    worker.open_editor(WindowHandle::X11(1), 1.0).unwrap();
    request.store(2, Ordering::Release);

    assert!(worker.service_main_thread().is_err());
    assert!(!worker.plugin.is_active());
    assert_eq!(worker.intrinsic_latency_frames(), None);
    worker.close_editor();
    assert_eq!(closes.load(Ordering::Relaxed), 1);
    drop(worker);
    assert_eq!(deactivations.load(Ordering::Relaxed), 0);
}

#[test]
fn active_worker_is_deactivated_once_on_drop() {
    let plugin = latency_fixture(64);
    let deactivations = Arc::clone(&plugin.deactivations);
    let worker = activate(plugin, 0, 1).unwrap();
    drop(worker);
    assert_eq!(deactivations.load(Ordering::Relaxed), 1);
}

#[test]
fn parameter_snapshot_rejects_oversized_count_before_native_metadata_reads() {
    let mut plugin = latency_fixture(0);
    plugin.parameter_count = usize::MAX;
    let reads = Arc::clone(&plugin.metadata_reads);
    let error = activate(plugin, 0, 1).err().unwrap();
    assert!(error.contains("exceeds 16 MiB"), "{error}");
    assert_eq!(reads.load(Ordering::Relaxed), 0);
}

#[test]
fn failed_main_thread_service_stops_worker_and_clears_latency() {
    for request_value in [2, 3] {
        let directory = mixrelay_test_support::FixtureDir::new("main-thread-failure").unwrap();
        let path = directory.join("audio");
        let layout = crate::AudioLayout {
            inputs: 1,
            outputs: 1,
            block_frames: 1,
        };
        let (_parent_audio, _mailbox) = crate::shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        let (_audio, control) = crate::shared_transport::worker(&path).unwrap();
        let plugin = latency_fixture(64);
        let request = Arc::clone(&plugin.service_request);
        let worker = Mutex::new(activate(plugin, 0, 1).unwrap());
        control
            .header()
            .worker_status
            .store(crate::WORKER_READY, Ordering::Release);
        crate::latency::publish_intrinsic_frames(control.header(), Some(64));
        request.store(request_value, Ordering::Release);

        assert!(crate::worker::broker::service_plugin_control(&control, &worker).is_err());
        assert_eq!(control.header().shutdown.load(Ordering::Acquire), 1);
        assert_eq!(
            control.header().worker_status.load(Ordering::Acquire),
            crate::WORKER_FAILED
        );
        assert_eq!(
            crate::latency::decode_intrinsic_frames(
                control
                    .header()
                    .intrinsic_latency_frames
                    .load(Ordering::Acquire)
            ),
            None
        );
        assert_eq!(worker.lock().unwrap().intrinsic_latency_frames(), None);
    }
}

#[test]
fn busy_dsp_defers_main_thread_request_without_consuming_it() {
    let directory = mixrelay_test_support::FixtureDir::new("main-thread-deferred").unwrap();
    let path = directory.join("audio");
    let layout = crate::AudioLayout {
        inputs: 1,
        outputs: 1,
        block_frames: 1,
    };
    let (_parent_audio, _mailbox) = crate::shared_transport::create(
        &path,
        layout,
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, control) = crate::shared_transport::worker(&path).unwrap();
    let plugin = latency_fixture(64);
    let request = Arc::clone(&plugin.service_request);
    let worker = Mutex::new(activate(plugin, 0, 1).unwrap());
    let dsp = worker.lock().unwrap();
    request.store(1, Ordering::Release);
    crate::worker::broker::service_plugin_control(&control, &worker).unwrap();
    assert_eq!(request.load(Ordering::Acquire), 1);
    drop(dsp);
    crate::worker::broker::service_plugin_control(&control, &worker).unwrap();
    assert_eq!(request.load(Ordering::Acquire), 0);
    worker.lock().unwrap().set_parameter(8, 0.75).unwrap();
}

#[test]
fn headless_main_thread_applies_parameter_mailbox_after_catalog_refresh() {
    let directory = mixrelay_test_support::FixtureDir::new("main-thread-parameter").unwrap();
    let path = directory.join("audio");
    let layout = crate::AudioLayout {
        inputs: 1,
        outputs: 1,
        block_frames: 1,
    };
    let (_parent_audio, _mailbox) = crate::shared_transport::create(
        &path,
        layout,
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let (_audio, control) = crate::shared_transport::worker(&path).unwrap();
    let plugin = latency_fixture(64);
    let request = Arc::clone(&plugin.service_request);
    let writes = Arc::clone(&plugin.parameter_writes);
    let worker = Mutex::new(activate(plugin, 0, 1).unwrap());
    let header = control.header();
    header.parameter_count.store(1, Ordering::Release);
    header.parameters[0].id.store(8, Ordering::Relaxed);
    header.parameters[0]
        .value_bits
        .store(0.75_f64.to_bits(), Ordering::Release);
    header.parameters[0].pending.store(true, Ordering::Release);
    header.parameters_pending.store(true, Ordering::Release);
    request.store(1, Ordering::Release);

    let dsp = worker.lock().unwrap();
    crate::worker::broker::service_plugin_control(&control, &worker).unwrap();
    assert!(header.parameters_pending.load(Ordering::Acquire));
    assert_eq!(writes.load(Ordering::Relaxed), 0);
    drop(dsp);
    crate::worker::broker::service_plugin_control(&control, &worker).unwrap();

    assert!(!header.parameters_pending.load(Ordering::Acquire));
    assert!(!header.parameters[0].pending.load(Ordering::Acquire));
    assert_eq!(writes.load(Ordering::Relaxed), 1);
    assert_eq!(header.process_faults.total(), 0);
    crate::worker::broker::service_plugin_control(&control, &worker).unwrap();
    assert_eq!(writes.load(Ordering::Relaxed), 1);
}

#[test]
fn native_state_request_waits_for_real_parameter_delivery_and_acknowledges_cancellation() {
    for cancelled in [false, true] {
        let directory = mixrelay_test_support::FixtureDir::new("state-after-parameters").unwrap();
        let path = directory.join("audio");
        let layout = crate::AudioLayout {
            inputs: 1,
            outputs: 1,
            block_frames: 1,
        };
        let (_parent, mailbox) = crate::shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        let (_audio, control) = crate::shared_transport::worker(&path).unwrap();
        let plugin = latency_fixture(64);
        let pending = Arc::clone(&plugin.pending_parameter);
        let captures = Arc::clone(&plugin.state_captures);
        let worker = Mutex::new(activate(plugin, 0, 1).unwrap());
        pending.store(true, Ordering::Release);
        mailbox
            .header()
            .worker_status
            .store(crate::WORKER_READY, Ordering::Release);
        let reader = thread::spawn(move || {
            mailbox
                .lock()
                .request_blob(crate::editor_surface::NATIVE_STATE, Duration::from_secs(2))
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while control.pending_request().is_none() {
            assert!(
                Instant::now() < deadline,
                "state reader must publish its request"
            );
            thread::yield_now();
        }

        crate::worker::broker::apply_pending_state_request(&control, &worker);
        assert!(control.pending_request().is_some());
        assert_eq!(captures.load(Ordering::Relaxed), 0);
        assert!(pending.load(Ordering::Acquire));
        if cancelled {
            assert_eq!(
                reader.join().unwrap(),
                Err(crate::ProcessorStateError::Timeout)
            );
            crate::worker::broker::apply_pending_state_request(&control, &worker);
        } else {
            worker.lock().unwrap().process(1).unwrap();
            crate::worker::broker::apply_pending_state_request(&control, &worker);
            assert_eq!(reader.join().unwrap(), Ok(Vec::new()));
        }

        assert!(control.pending_request().is_none());
        assert_eq!(captures.load(Ordering::Relaxed), usize::from(!cancelled));
        assert_eq!(pending.load(Ordering::Acquire), cancelled);
    }
}

#[test]
fn native_state_load_keeps_latency_unknown_until_known_process_snapshot() {
    let mut plugin = latency_fixture(29);
    plugin.next_process_action = Some(FixtureProcessAction::ReportUnknownLatency);
    let initial = crate::state::InitialState {
        parameters: serde_json::json!({}),
        native: Some(vec![1]),
    };
    let mut worker = activate_with_initial_state(plugin, 0, 1, false, &initial).unwrap();

    assert_eq!(worker.intrinsic_latency_frames(), None);
    assert_unknown_until_known(&mut worker, 29);
}

#[test]
fn editor_mutation_keeps_latency_unknown_until_known_process_snapshot() {
    let mut plugin = latency_fixture(31);
    plugin.info.has_editor = true;
    plugin.next_process_action = Some(FixtureProcessAction::ReportUnknownLatency);
    let initial = crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap();
    let mut worker = activate_with_initial_state(plugin, 0, 1, true, &initial).unwrap();

    assert_eq!(worker.intrinsic_latency_frames(), Some(0));
    worker.open_editor(WindowHandle::X11(1), 1.0).unwrap();
    assert_eq!(worker.intrinsic_latency_frames(), None);
    assert_unknown_until_known(&mut worker, 31);
}

#[test]
fn process_status_and_call_errors_invalidate_until_known_snapshot() {
    for action in [
        FixtureProcessAction::StatusErrorThenUnknown,
        FixtureProcessAction::CallErrorThenUnknown,
    ] {
        let mut plugin = latency_fixture(37);
        plugin.next_process_action = Some(action);
        let mut worker = activate(plugin, 0, 1).unwrap();

        assert_eq!(worker.intrinsic_latency_frames(), Some(0));
        assert!(worker.process(1).is_err());
        assert_eq!(worker.intrinsic_latency_frames(), None);
        assert_unknown_until_known(&mut worker, 37);
    }
}

#[test]
fn worker_revalidates_saved_topology_before_activation() {
    let selected = layouts::describe(&BusLayout::stereo()).unwrap();
    let activation = WorkerActivation {
        editor_allowed: false,
        sample_rate: 48_000.0,
        max_block_size: 32,
    };
    let initial = crate::state::InitialState::resolve(serde_json::json!({}), None).unwrap();
    for channels in [0, 65, u32::MAX] {
        let plugin = fixture(vec![channel_layout(channels)]);
        let activations = Arc::clone(&plugin.activations);
        assert!(
            RackWorkerPlugin::activate(Box::new(plugin), &selected, &activation, &initial,)
                .is_err()
        );
        assert_eq!(activations.load(Ordering::Relaxed), 0);
    }
    let plugin = fixture(Vec::new());
    let activations = Arc::clone(&plugin.activations);
    assert!(
        RackWorkerPlugin::activate(Box::new(plugin), &selected, &activation, &initial,).is_err()
    );
    assert_eq!(activations.load(Ordering::Relaxed), 0);
}

#[test]
fn worker_rejects_over_limit_inputs_outputs_and_bus_sum_overflow() {
    use truce_rack_core::bus::{Bus, ChannelConfig};
    for input_side in [true, false] {
        for counts in [[64, 1], [u32::MAX, 2]] {
            let mut layout = BusLayout::stereo();
            let buses = if input_side {
                &mut layout.inputs
            } else {
                &mut layout.outputs
            };
            buses.clear();
            buses.extend(counts.map(|count| Bus::main("Audio", ChannelConfig::Discrete(count))));
            assert!(select_effect_layout(&[layout.clone()]).is_none());
            let plugin = fixture(vec![layout]);
            let activations = Arc::clone(&plugin.activations);
            assert!(activate(plugin, 0, 32).is_err());
            assert_eq!(activations.load(Ordering::Relaxed), 0);
        }
    }
}
