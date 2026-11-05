//! Scripted native plug-in shared by probe and worker tests.
//!
//! Each field scripts or records one native behavior: layout activation and rejection,
//! deactivation refusal, latency reports, parameter metadata, state and editor calls.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use truce_rack_core::{
    Plugin, PluginCore,
    buffer::AudioBuffer,
    bus::BusLayout,
    editor::{PluginEditor, WindowHandle},
    error::{Error, Result as PluginResult},
    events::EventList,
    info::{ParameterFlags, ParameterInfo, PluginCategory, PluginInfo, PresetInfo},
    plugin::{MainThreadUpdate, ProcessContext, ProcessStatus},
};

#[derive(Clone, Copy)]
pub(super) enum FixtureProcessAction {
    ReportUnknownLatency,
    StatusErrorThenUnknown,
    CallErrorThenUnknown,
    NonFinite(f32),
    PartialFailure,
    OutputEvents(usize),
}

#[derive(Clone, Copy)]
pub(super) enum FixtureActivationFailure {
    Reusable,
    Fatal,
    StillActive,
}

pub(super) struct ChannelFixture {
    pub(super) info: PluginInfo,
    pub(super) layouts: Vec<BusLayout>,
    pub(super) active: Option<BusLayout>,
    pub(super) activations: Arc<AtomicUsize>,
    pub(super) deactivations: Arc<AtomicUsize>,
    pub(super) latency_frames: Option<u32>,
    pub(super) latency_after_process: Option<u32>,
    pub(super) next_process_action: Option<FixtureProcessAction>,
    pub(super) editor_open: bool,
    pub(super) service_request: Arc<AtomicUsize>,
    pub(super) main_thread: std::thread::ThreadId,
    pub(super) parameter_id: u32,
    pub(super) parameter_count: usize,
    pub(super) metadata_reads: Arc<AtomicUsize>,
    pub(super) metadata_error: bool,
    pub(super) editor_closes: Arc<AtomicUsize>,
    pub(super) parameter_writes: Arc<AtomicUsize>,
    pub(super) pending_parameter: Arc<AtomicBool>,
    pub(super) state_captures: Arc<AtomicUsize>,
    pub(super) activation_failure: Option<(u32, FixtureActivationFailure)>,
    pub(super) refuse_deactivation_after: Option<usize>,
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

pub(super) fn channel_layout(channels: u32) -> BusLayout {
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

pub(super) fn fixture(layouts: Vec<BusLayout>) -> ChannelFixture {
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
