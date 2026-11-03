use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use truce_rack_core::{
    PluginCore,
    editor::{PluginEditor, WindowHandle},
    error::{Error, Result as PluginResult},
    info::{ParameterFlags, ParameterInfo, PresetInfo},
};

use super::super::{RackFormat, describe_loaded_plugin, select_effect_layout};
use super::*;

#[path = "channel_tests/pcm.rs"]
mod pcm;
use std::path::PathBuf;
use truce_rack_core::{
    bus::BusLayout,
    info::{PluginCategory, PluginInfo},
};

#[derive(Clone, Copy)]
enum FixtureProcessAction {
    ReportUnknownLatency,
    StatusErrorThenUnknown,
    CallErrorThenUnknown,
    NonFinite(f32),
    PartialFailure,
    OutputEvents(usize),
}

#[derive(Clone, Copy)]
enum FixtureActivationFailure {
    Reusable,
    Fatal,
    StillActive,
}

struct ChannelFixture {
    info: PluginInfo,
    layouts: Vec<BusLayout>,
    active: Option<BusLayout>,
    activations: Arc<AtomicUsize>,
    deactivations: Arc<AtomicUsize>,
    latency_frames: Option<u32>,
    latency_after_process: Option<u32>,
    next_process_action: Option<FixtureProcessAction>,
    editor_open: bool,
    service_request: Arc<AtomicUsize>,
    main_thread: std::thread::ThreadId,
    parameter_id: u32,
    parameter_count: usize,
    metadata_reads: Arc<AtomicUsize>,
    metadata_error: bool,
    editor_closes: Arc<AtomicUsize>,
    parameter_writes: Arc<AtomicUsize>,
    pending_parameter: Arc<AtomicBool>,
    state_captures: Arc<AtomicUsize>,
    activation_failure: Option<(u32, FixtureActivationFailure)>,
    refuse_deactivation_after: Option<usize>,
}

impl PluginCore for ChannelFixture {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn active_layout(&self) -> Option<&BusLayout> {
        self.active.as_ref()
    }

    fn supported_layouts(&self) -> &[BusLayout] {
        &self.layouts
    }

    fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    fn parameter_info(&self, index: usize) -> PluginResult<ParameterInfo> {
        self.metadata_reads.fetch_add(1, Ordering::Relaxed);
        if index != 0 || self.metadata_error {
            return Err(Error::InvalidParameter(index));
        }
        Ok(ParameterInfo {
            id: self.parameter_id,
            name: "Latency trigger".to_owned(),
            short_name: "Latency".to_owned(),
            unit: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.5,
            step_count: 0,
            flags: ParameterFlags::empty(),
        })
    }

    fn parameter_value(&self, index: usize) -> PluginResult<f64> {
        if index == 0 {
            Ok(0.5)
        } else {
            Err(Error::InvalidParameter(index))
        }
    }

    fn parameter_value_string(&self, index: usize, _value: f64) -> PluginResult<String> {
        Err(Error::InvalidParameter(index))
    }

    fn set_parameter(&mut self, index: usize, _value: f64) -> PluginResult<()> {
        assert_eq!(std::thread::current().id(), self.main_thread);
        if index != 0 {
            return Err(Error::InvalidParameter(index));
        }
        self.parameter_writes.fetch_add(1, Ordering::Relaxed);
        self.latency_frames = None;
        Ok(())
    }

    fn preset_count(&self) -> usize {
        0
    }

    fn preset_info(&self, index: usize) -> PluginResult<PresetInfo> {
        Err(Error::InvalidParameter(index))
    }

    fn load_preset(&mut self, _preset_number: i32) -> PluginResult<()> {
        Ok(())
    }

    fn save_state(&self) -> PluginResult<Vec<u8>> {
        self.state_captures.fetch_add(1, Ordering::Relaxed);
        Ok(Vec::new())
    }

    fn has_pending_parameter_changes(&self) -> bool {
        self.pending_parameter.load(Ordering::Acquire)
    }

    fn load_state(&mut self, bytes: &[u8]) -> PluginResult<()> {
        assert!(self.active.is_none(), "restore state before bus activation");
        if bytes == [255] {
            return Err(Error::Other("invalid fixture state".into()));
        }
        self.latency_frames = None;
        if bytes == [8] {
            self.parameter_id = 8;
        } else if bytes == [254] {
            self.layouts[0].outputs[0].kind = truce_rack_core::bus::BusKind::Auxiliary;
        }
        Ok(())
    }

    fn service_main_thread(&mut self) -> PluginResult<MainThreadUpdate> {
        assert_eq!(std::thread::current().id(), self.main_thread);
        match self.service_request.swap(0, Ordering::AcqRel) {
            0 => Ok(MainThreadUpdate::Unchanged),
            1 => {
                self.parameter_id = 8;
                self.latency_frames = Some(64);
                Ok(MainThreadUpdate::ParametersChanged)
            }
            2 => {
                self.active = None;
                Err(Error::Other("fixture restart failed".to_owned()))
            }
            3 => {
                self.metadata_error = true;
                Ok(MainThreadUpdate::ParametersChanged)
            }
            request => panic!("unexpected fixture service request {request}"),
        }
    }

    fn latency_frames(&self) -> Option<u32> {
        self.latency_frames
    }

    fn editor(&mut self) -> Option<&mut dyn PluginEditor> {
        self.info.has_editor.then_some(self)
    }

    fn activate(&mut self, layout: BusLayout, _rate: f64, _frames: usize) -> PluginResult<()> {
        self.activations.fetch_add(1, Ordering::Relaxed);
        if let Some((channels, failure)) = self.activation_failure
            && layout.total_input_channels() == channels
        {
            match failure {
                FixtureActivationFailure::Fatal => {
                    return Err(Error::Other("native rollback failed".into()));
                }
                FixtureActivationFailure::StillActive => self.active = Some(layout),
                FixtureActivationFailure::Reusable => {}
            }
            return Err(Error::ActivationRejected {
                reason: format!("fixture rejects {channels} channels"),
            });
        }
        self.active = Some(
            if self.layouts[0]
                .outputs
                .first()
                .is_some_and(|bus| bus.kind == truce_rack_core::bus::BusKind::Auxiliary)
            {
                self.layouts[0].clone()
            } else {
                layout
            },
        );
        Ok(())
    }

    fn deactivate(&mut self) {
        assert!(
            self.active.is_some(),
            "cannot deactivate an inactive fixture"
        );
        let count = self.deactivations.fetch_add(1, Ordering::Relaxed) + 1;
        if self.refuse_deactivation_after != Some(count) {
            self.active = None;
        }
    }

    fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

impl PluginEditor for ChannelFixture {
    fn open(&mut self, _parent: WindowHandle, _scale: f64) -> PluginResult<()> {
        self.editor_open = true;
        self.latency_frames = None;
        Ok(())
    }

    fn close(&mut self) {
        assert_eq!(std::thread::current().id(), self.main_thread);
        self.editor_closes.fetch_add(1, Ordering::Relaxed);
        self.editor_open = false;
        self.latency_frames = None;
    }

    fn is_open(&self) -> bool {
        self.editor_open
    }

    fn size(&self) -> Option<(u32, u32)> {
        self.editor_open.then_some((640, 480))
    }

    fn is_resizable(&self) -> bool {
        true
    }

    fn set_size(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.editor_open.then_some((width, height))
    }

    fn show(&mut self) {
        self.editor_open = true;
    }

    fn hide(&mut self) {
        self.editor_open = false;
    }
}

impl Plugin<f32> for ChannelFixture {
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        _events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> PluginResult<ProcessStatus> {
        match self.next_process_action.take() {
            Some(FixtureProcessAction::ReportUnknownLatency) => self.latency_frames = None,
            Some(FixtureProcessAction::StatusErrorThenUnknown) => {
                self.next_process_action = Some(FixtureProcessAction::ReportUnknownLatency);
                return Ok(ProcessStatus::Error);
            }
            Some(FixtureProcessAction::CallErrorThenUnknown) => {
                self.next_process_action = Some(FixtureProcessAction::ReportUnknownLatency);
                return Err(Error::Other("fixture process failure".to_owned()));
            }
            Some(FixtureProcessAction::NonFinite(value)) => {
                buffer.main_outputs()[0][0] = value;
                return Ok(ProcessStatus::Continue);
            }
            Some(FixtureProcessAction::PartialFailure) => {
                buffer.main_outputs()[0][0] = 9.0;
                return Err(Error::NotActivated);
            }
            Some(FixtureProcessAction::OutputEvents(count)) => {
                use truce_rack_core::events::{Event, EventBody, TransportFlag};
                for _ in 0..count {
                    if !context.output_events.try_push(Event {
                        sample_offset: 0,
                        body: EventBody::TransportFlag(TransportFlag::PlayStart),
                    }) {
                        return Ok(ProcessStatus::Error);
                    }
                }
            }
            None => self.latency_frames = self.latency_after_process,
        }
        self.pending_parameter.store(false, Ordering::Release);
        assert_eq!(
            buffer.total_input_channels(),
            buffer.total_output_channels()
        );
        for index in 0..buffer.total_input_channels() {
            let sample = buffer.main_inputs()[index][0];
            buffer.main_outputs()[index].fill(sample);
        }
        Ok(ProcessStatus::Continue)
    }
}

fn channel_layout(channels: u32) -> BusLayout {
    use truce_rack_core::bus::{Bus, ChannelConfig};
    let mut layout = BusLayout::new();
    layout
        .inputs
        .push(Bus::main("Input", ChannelConfig::Discrete(channels)));
    layout
        .outputs
        .push(Bus::main("Output", ChannelConfig::Discrete(channels)));
    layout
}

fn fixture(layouts: Vec<BusLayout>) -> ChannelFixture {
    let info = PluginInfo {
        name: "Channel fixture".to_owned(),
        vendor: "MixRelay".to_owned(),
        version: 1,
        category: PluginCategory::Effect,
        path: PathBuf::new(),
        unique_id: "channel-fixture".to_owned(),
        format: "fixture",
        has_editor: false,
        accepts_midi: false,
    };
    ChannelFixture {
        info,
        layouts,
        active: None,
        activations: Arc::new(AtomicUsize::new(0)),
        deactivations: Arc::new(AtomicUsize::new(0)),
        latency_frames: None,
        latency_after_process: None,
        next_process_action: None,
        editor_open: false,
        service_request: Arc::new(AtomicUsize::new(0)),
        main_thread: std::thread::current().id(),
        parameter_id: 7,
        parameter_count: 1,
        metadata_reads: Arc::new(AtomicUsize::new(0)),
        metadata_error: false,
        editor_closes: Arc::new(AtomicUsize::new(0)),
        parameter_writes: Arc::new(AtomicUsize::new(0)),
        pending_parameter: Arc::new(AtomicBool::new(false)),
        state_captures: Arc::new(AtomicUsize::new(0)),
        activation_failure: None,
        refuse_deactivation_after: None,
    }
}

#[test]
fn catalog_omits_rejected_layout_and_chooses_only_a_verified_default() {
    let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
    plugin.activation_failure = Some((2, FixtureActivationFailure::Reusable));
    let locator = RackLocator::from_info(RackFormat::Au, plugin.info.clone());
    let record = describe_loaded_plugin(locator, &mut plugin).unwrap();
    assert_eq!(record.descriptor.layouts.len(), 1);
    assert_eq!(record.descriptor.layouts[0].total_input_channels(), Ok(1));
    assert_eq!(
        record.descriptor.default_layout_id,
        record.descriptor.layouts[0].layout_id
    );
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 3);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 2);
    assert!(!plugin.is_active());
}

#[test]
fn catalog_preserves_cause_when_every_layout_is_rejected() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    plugin.activation_failure = Some((2, FixtureActivationFailure::Reusable));
    let error = layouts::verified_layouts(&mut plugin).unwrap_err();
    assert!(error.contains("no usable audio effect layout"), "{error}");
    assert!(error.contains("fixture rejects 2 channels"), "{error}");
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 0);
}

#[test]
fn catalog_does_not_hide_rollback_or_active_rejection_failures() {
    for failure in [
        FixtureActivationFailure::Fatal,
        FixtureActivationFailure::StillActive,
    ] {
        let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
        plugin.activation_failure = Some((2, failure));
        assert!(layouts::verified_layouts(&mut plugin).is_err());
        assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
        assert!(!plugin.is_active());
    }
}

#[test]
fn catalog_requires_successful_deactivation_before_the_next_configuration() {
    let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
    plugin.refuse_deactivation_after = Some(1);
    let error = layouts::verified_layouts(&mut plugin).unwrap_err();
    assert!(error.contains("did not deactivate"), "{error}");
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
}

#[test]
fn catalog_does_not_publish_after_final_default_teardown_fails() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    plugin.refuse_deactivation_after = Some(2);
    let locator = RackLocator::from_info(RackFormat::Au, plugin.info.clone());
    let error = describe_loaded_plugin(locator, &mut plugin).unwrap_err();
    assert!(
        error.contains("default layout did not deactivate"),
        "{error}"
    );
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 2);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 2);
    assert!(plugin.is_active());
}

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

#[test]
fn large_catalog_still_deduplicates_and_rejects_over_limit_channels() {
    let mut plugin = fixture(vec![BusLayout::stereo(); 80]);
    assert_eq!(layouts::verified_layouts(&mut plugin).unwrap().len(), 1);
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 1);
    let invalid = plugin.layouts.last_mut().unwrap();
    invalid.outputs[0].channels = truce_rack_core::bus::ChannelConfig::Discrete(65);
    assert!(layouts::verified_layouts(&mut plugin).is_err());
    assert!(!plugin.is_active());
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
