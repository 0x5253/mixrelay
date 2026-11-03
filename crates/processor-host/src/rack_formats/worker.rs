//! Owns one activated native instance, its bounded planar scratch and native processing contract.

use super::{CHANNEL_SLOTS, EditorSnapshot, RackLocator, layout_channels, layouts, load_plugin};

// Native event producers have a fixed per-block budget, reserved before activation.
const OUTPUT_EVENT_CAPACITY: usize = 4096;
use arrayvec::ArrayVec;
use mixrelay_audio_graph::ProviderPorts;
use std::mem::ManuallyDrop;
use truce_rack_core::{
    Plugin, WindowHandle,
    buffer::{AudioBuffer, BusRange},
    events::EventList,
    plugin::{MainThreadUpdate, ProcessContext, ProcessStatus},
};

#[cfg(test)]
#[path = "channel_tests.rs"]
mod channel_tests;

pub(crate) struct RackWorkerPlugin {
    pub(super) plugin: Box<dyn Plugin<f32>>,
    editor_allowed: bool,
    input_storage: Vec<Vec<f32>>,
    output_storage: Vec<Vec<f32>>,
    input_ranges: Vec<BusRange>,
    output_ranges: Vec<BusRange>,
    dry_mapping: Vec<Option<usize>>,
    pub(super) parameter_ranges: Vec<ParameterRange>,
    input_events: EventList,
    output_events: EventList,
    sample_rate: f64,
    max_block_size: usize,
    latency_gate: crate::latency::PublicationGate,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ParameterRange {
    pub(super) id: u32,
    pub(super) index: usize,
    minimum: f64,
    maximum: f64,
}

struct WorkerActivation {
    editor_allowed: bool,
    sample_rate: f64,
    max_block_size: usize,
}

/// Failed native processing never authorizes publication of its output or cached latency.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProcessFailure {
    #[error("processor block exceeds its prepared geometry")]
    InvalidFrames,
    #[error("processor returned non-finite PCM")]
    NonFinite,
    #[error("processor reported a fatal process error")]
    Fatal,
    #[error("native processor failed: {0}")]
    Native(#[from] truce_rack_core::error::Error),
}

impl ProcessFailure {
    pub(crate) const fn native_cause(&self) -> Option<&truce_rack_core::Error> {
        match self {
            Self::Native(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) const fn kind(&self) -> crate::process_fault::FaultKind {
        use crate::process_fault::FaultKind;
        match self {
            Self::InvalidFrames => FaultKind::InvalidGeometry,
            Self::NonFinite => FaultKind::NonFinitePcm,
            Self::Fatal => FaultKind::ProcessStatus,
            Self::Native(_) => FaultKind::NativeCall,
        }
    }
}

impl RackWorkerPlugin {
    pub(crate) fn load(
        locator: &RackLocator,
        selected: &ProviderPorts,
        sample_rate: f64,
        max_block_size: usize,
        initial: &crate::state::InitialState,
    ) -> Result<Self, String> {
        Self::activate(
            load_plugin(locator, selected)?,
            selected,
            &WorkerActivation {
                editor_allowed: locator.has_editor,
                sample_rate,
                max_block_size,
            },
            initial,
        )
    }

    fn activate(
        mut plugin: Box<dyn Plugin<f32>>,
        selected: &ProviderPorts,
        activation: &WorkerActivation,
        initial: &crate::state::InitialState,
    ) -> Result<Self, String> {
        let layout = layouts::native_layout(plugin.supported_layouts(), selected)?;
        // Resolve the exact native identity and ordered topology before allocating worker
        // buffers. Enumeration positions and equal channel totals do not identify a layout.
        let (input_channels, output_channels) = layout_channels(&layout)?;
        let input_ranges = bus_ranges(&layout.inputs);
        let output_ranges = bus_ranges(&layout.outputs);
        let dry_mapping =
            crate::dry_delay::dry_channel_mapping(selected).map_err(|error| error.to_string())?;
        let input_storage = planar_storage(usize::from(input_channels), activation.max_block_size)?;
        let output_storage =
            planar_storage(usize::from(output_channels), activation.max_block_size)?;
        let output_events = EventList::try_with_capacity(OUTPUT_EVENT_CAPACITY)
            .map_err(|error| format!("cannot allocate processor output events: {error}"))?;
        plugin
            .activate_with_state(
                layout,
                activation.sample_rate,
                activation.max_block_size,
                initial.native.as_deref(),
            )
            .map_err(|error| error.to_string())?;
        let mut worker = Self {
            plugin,
            editor_allowed: activation.editor_allowed,
            input_storage,
            output_storage,
            input_ranges,
            output_ranges,
            dry_mapping,
            parameter_ranges: Vec::new(),
            input_events: EventList::new(),
            output_events,
            sample_rate: activation.sample_rate,
            max_block_size: activation.max_block_size,
            latency_gate: crate::latency::PublicationGate::default(),
        };
        layouts::verify_active(worker.plugin.as_ref(), selected)?;
        worker.apply_initial_parameters(initial)?;
        Ok(worker)
    }

    pub(crate) fn input_channels(&self) -> usize {
        self.input_storage.len()
    }

    pub(crate) fn output_channels(&self) -> usize {
        self.output_storage.len()
    }

    pub(crate) fn intrinsic_latency_frames(&self) -> Option<u32> {
        self.latency_gate.cached(self.plugin.latency_frames())
    }

    /// Services format-owned requests on the instance's fixed main thread, with exclusive
    /// access against DSP. A failed restart or metadata refresh must stop this worker.
    pub(crate) fn service_main_thread(&mut self) -> Result<(), String> {
        let result = self
            .plugin
            .service_main_thread()
            .map_err(|error| error.to_string());
        match result {
            Ok(MainThreadUpdate::Unchanged) => Ok(()),
            Ok(MainThreadUpdate::ParametersChanged) => {
                self.latency_gate.invalidate();
                self.refresh_parameter_ranges()
            }
            Err(error) => {
                self.latency_gate.invalidate();
                Err(error)
            }
        }
    }

    fn refresh_parameter_ranges(&mut self) -> Result<(), String> {
        // Third-party metadata controls this count. Bound the snapshot before allocation;
        // rejecting an oversized instance is explicit, never a truncated parameter list.
        const SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
        let count = self.plugin.parameter_count();
        if count > SNAPSHOT_BYTES / std::mem::size_of::<ParameterRange>() {
            return Err("processor parameter snapshot exceeds 16 MiB".to_owned());
        }
        let mut ranges = Vec::new();
        ranges
            .try_reserve_exact(count)
            .map_err(|error| format!("cannot allocate processor parameter snapshot: {error}"))?;
        for index in 0..count {
            let parameter = self
                .plugin
                .parameter_info(index)
                .map_err(|error| format!("cannot read processor parameter {index}: {error}"))?;
            ranges.push(ParameterRange {
                id: parameter.id,
                index,
                minimum: parameter.min,
                maximum: parameter.max,
            });
        }
        self.parameter_ranges = ranges;
        Ok(())
    }

    pub(crate) fn inputs_mut(&mut self) -> &mut [Vec<f32>] {
        &mut self.input_storage
    }

    pub(crate) fn outputs(&self) -> &[Vec<f32>] {
        &self.output_storage
    }

    #[cfg(test)]
    pub(crate) fn clear_outputs(&mut self) {
        for channel in &mut self.output_storage {
            channel.fill(0.0);
        }
    }

    pub(crate) fn bypass(&mut self, frames: usize) {
        for (channel_index, output) in self.output_storage.iter_mut().enumerate() {
            if let Some(input) =
                self.dry_mapping[channel_index].and_then(|index| self.input_storage.get(index))
            {
                output[..frames].copy_from_slice(&input[..frames]);
            } else {
                output[..frames].fill(0.0);
            }
        }
    }

    pub(crate) fn set_parameter(&mut self, id: u32, normalized: f64) -> Result<(), String> {
        self.latency_gate.invalidate();
        let range = self
            .parameter_ranges
            .iter()
            .find(|range| range.id == id)
            .copied()
            .ok_or_else(|| format!("processor parameter {id} is unavailable"))?;
        let value = range.minimum + normalized.clamp(0.0, 1.0) * (range.maximum - range.minimum);
        self.plugin
            .set_parameter(range.index, value)
            .map_err(|error| error.to_string())
    }

    /// Control/editor broker only. The native instance is serialized against DSP and editor
    /// mutation by the caller. Formats with unsupported state APIs deliberately return errors.
    pub(crate) fn save_state(&self) -> Result<Vec<u8>, crate::ProcessorStateError> {
        if self.plugin.has_pending_parameter_changes() {
            return Err(crate::ProcessorStateError::Pending);
        }
        self.plugin
            .save_state()
            .map_err(|_| crate::ProcessorStateError::NativeFailure)
    }

    pub(crate) fn process(&mut self, frames: usize) -> Result<(), ProcessFailure> {
        if frames == 0 || frames > self.max_block_size {
            self.latency_gate.process_failed();
            return Err(ProcessFailure::InvalidFrames);
        }
        // Activation validates the channel cap. Inline vectors initialize only live channels
        // without allocating the temporary slice-of-slices required by the host API.
        let mut input_refs = ArrayVec::<&[f32], CHANNEL_SLOTS>::new();
        for channel in &self.input_storage {
            input_refs.push(&channel[..frames]);
        }
        // AudioBuffer ties the outer borrow to its inner mutable references. Suppress the
        // container's drop check: it owns only borrowed slices, never the channel allocations.
        let mut output_refs = ManuallyDrop::new(ArrayVec::<&mut [f32], CHANNEL_SLOTS>::new());
        for channel in &mut self.output_storage {
            channel[..frames].fill(0.0);
            output_refs.push(&mut channel[..frames]);
        }
        let mut buffer = AudioBuffer::new(
            &input_refs,
            &mut output_refs,
            frames,
            &self.input_ranges,
            &self.output_ranges,
        );
        self.output_events.clear();
        let mut context = ProcessContext {
            sample_rate: self.sample_rate,
            max_block_size: self.max_block_size,
            transport: None,
            output_events: &mut self.output_events,
        };
        let status = match self
            .plugin
            .process(&mut buffer, &self.input_events, &mut context)
        {
            Ok(status) => status,
            Err(error) => {
                self.latency_gate.process_failed();
                return Err(ProcessFailure::Native(error));
            }
        };
        match status {
            ProcessStatus::Error => {
                self.latency_gate.process_failed();
                Err(ProcessFailure::Fatal)
            }
            ProcessStatus::Continue | ProcessStatus::Sleep | ProcessStatus::Tail { .. } => {
                if self
                    .output_storage
                    .iter()
                    .any(|channel| channel[..frames].iter().any(|sample| !sample.is_finite()))
                {
                    self.latency_gate.process_failed();
                    return Err(ProcessFailure::NonFinite);
                }
                self.latency_gate
                    .process_succeeded(self.plugin.latency_frames());
                Ok(())
            }
        }
    }

    pub(crate) fn has_editor(&self) -> bool {
        self.editor_allowed && self.plugin.info().has_editor
    }

    pub(crate) fn open_editor(
        &mut self,
        parent: WindowHandle,
        scale: f64,
    ) -> Result<EditorSnapshot, String> {
        self.latency_gate.invalidate();
        let editor = self
            .plugin
            .editor()
            .ok_or_else(|| "processor does not expose a native editor".to_owned())?;
        editor
            .open(parent, scale)
            .map_err(|error| error.to_string())?;
        editor.show();
        Ok(EditorSnapshot {
            open: editor.is_open(),
            size: editor.size(),
            resizable: editor.is_resizable(),
        })
    }

    pub(crate) fn close_editor(&mut self) {
        self.latency_gate.invalidate();
        if let Some(editor) = self.plugin.editor() {
            editor.hide();
            editor.close();
        }
    }

    pub(crate) fn tick_editor(&mut self) -> EditorSnapshot {
        self.latency_gate.invalidate();
        let Some(editor) = self.plugin.editor() else {
            return EditorSnapshot::default();
        };
        editor.on_idle();
        EditorSnapshot {
            open: editor.is_open(),
            size: editor.size(),
            resizable: editor.is_resizable(),
        }
    }

    pub(crate) fn resize_editor(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.latency_gate.invalidate();
        self.plugin.editor()?.set_size(width, height)
    }

    fn apply_initial_parameters(
        &mut self,
        initial: &crate::state::InitialState,
    ) -> Result<(), String> {
        self.service_main_thread()?;
        // Activation and native state restoration may both change indexed metadata. Resolve
        // persisted stable IDs only after these lifecycle calls have completed.
        self.refresh_parameter_ranges()?;
        let Some(object) = initial.parameters.as_object() else {
            return Ok(());
        };
        if let Some(values) = object
            .get("parameterValues")
            .and_then(serde_json::Value::as_object)
        {
            for (id, value) in values {
                let id = id
                    .parse::<u32>()
                    .map_err(|error| format!("invalid processor parameter ID {id}: {error}"))?;
                let value = value
                    .as_f64()
                    .ok_or_else(|| format!("processor parameter {id} is not numeric"))?;
                self.set_parameter(id, value)?;
            }
        }
        Ok(())
    }
}

impl Drop for RackWorkerPlugin {
    fn drop(&mut self) {
        if self.plugin.is_active() {
            self.plugin.deactivate();
        }
    }
}

fn bus_ranges(buses: &[truce_rack_core::bus::Bus]) -> Vec<BusRange> {
    let mut offset = 0_usize;
    buses
        .iter()
        .map(|bus| {
            let channels = usize::try_from(bus.channels.count()).unwrap_or(usize::MAX);
            let range = BusRange::new(offset, channels);
            offset = offset.saturating_add(channels);
            range
        })
        .collect()
}

fn planar_storage(channels: usize, frames: usize) -> Result<Vec<Vec<f32>>, String> {
    if channels == 0
        || channels > CHANNEL_SLOTS
        || frames == 0
        || frames > mixrelay_audio_io::formats::MAX_BUFFER_FRAMES
    {
        return Err("processor scratch exceeds supported geometry".to_owned());
    }
    let mut storage = mixrelay_audio_io::pcm_buffer::reserve(channels)
        .map_err(|error| format!("cannot allocate processor channel storage: {error}"))?;
    for _ in 0..channels {
        storage.push(
            mixrelay_audio_io::pcm_buffer::zeroed(frames)
                .map_err(|error| format!("cannot allocate processor PCM scratch: {error}"))?,
        );
    }
    Ok(storage)
}
