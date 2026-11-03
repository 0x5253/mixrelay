//! Service controller changes, state, activation, and latency on the creator thread.

use super::Vst3Plugin;
use crate::{
    activation, bus_layout, host_context,
    latency::{
        LatencyServiceAction, latency_refresh_allowed, latency_revision_is_current,
        latency_service_action, refresh_latency_cache,
    },
    metadata::vst3_param_info_to_rack,
    plugin_state,
};
use truce_rack_core::{
    bus::BusLayout,
    error::{Error, Result},
    info::{ParameterInfo, PluginInfo, PresetInfo},
    plugin::{MainThreadUpdate, PluginCore},
};
use vst3::Steinberg::Vst::{
    IAudioProcessorTrait, IComponentTrait, IEditControllerTrait, ProcessModes_, ProcessSetup,
    SymbolicSampleSizes_,
};

impl Vst3Plugin {
    fn restart_for_latency_change(&mut self) -> Result<()> {
        self.processing.latency_frames = None;
        let Some(layout) = self.processing.active_layout.clone() else {
            return Ok(());
        };
        let mut setup = self
            .processing
            .activation_setup
            .ok_or_else(|| Error::Other("VST3 active instance lost its processing setup".into()))?;
        let revision = self.native.host_services.latency_revision();
        let mut state = activation::RestartState {
            setup: &mut setup,
            processing: &mut self.processing.processing,
            latency_frames: &mut self.processing.latency_frames,
        };
        let result = activation::restart(
            &self.native.component,
            &self.native.processor,
            &layout,
            &mut state,
        );
        result?;
        self.processing.active_layout = Some(layout);
        self.processing.activation_setup = Some(setup);
        self.publish_latency_revision_if_current(revision);
        Ok(())
    }

    fn publish_latency_revision_if_current(&mut self, revision: u64) -> bool {
        if !latency_revision_is_current(
            revision,
            self.native.host_services.latency_revision(),
            self.has_pending_parameter_changes(),
        ) {
            self.processing.latency_frames = None;
            return false;
        }
        self.native.host_services.publish_latency_revision(revision);
        if !latency_revision_is_current(
            revision,
            self.native.host_services.latency_revision(),
            self.has_pending_parameter_changes(),
        ) {
            self.processing.latency_frames = None;
            return false;
        }
        true
    }

    fn refresh_latency_snapshot(&mut self) {
        if self.native.host_services.latency_snapshot_valid() {
            return;
        }
        self.processing.latency_frames = None;
        if !latency_refresh_allowed(
            self.processing.active_layout.is_some()
                && self.processing.processing
                && self.processing.activation_setup.is_some(),
            self.editor.open,
            self.has_pending_parameter_changes(),
        ) {
            return;
        }

        let revision = self.native.host_services.latency_revision();
        let processor = &self.native.processor;
        let host_services = &self.native.host_services;
        let input_parameters = &self.processing.input_parameters;
        let mut snapshot = None;
        let published = refresh_latency_cache(
            &mut snapshot,
            revision,
            || {
                // SAFETY: this cache refresh runs only on the fixed creator thread after
                // setupProcessing and while the worker serializes access to the VST3 object.
                // Native latency getters are never called from process().
                unsafe { processor.getLatencySamples() }
            },
            |captured_revision| {
                let pending =
                    input_parameters.has_pending_changes() || host_services.has_pending_edits();
                if !latency_revision_is_current(
                    captured_revision,
                    host_services.latency_revision(),
                    pending,
                ) {
                    return false;
                }
                host_services.publish_latency_revision(captured_revision);
                let pending =
                    input_parameters.has_pending_changes() || host_services.has_pending_edits();
                latency_revision_is_current(
                    captured_revision,
                    host_services.latency_revision(),
                    pending,
                )
            },
        );
        if published {
            self.processing.latency_frames = snapshot;
        }
    }

    fn service_requests(&mut self, flags: u32) -> Result<MainThreadUpdate> {
        let requests = host_context::parse_restart_flags(flags)?;
        let services = &self.native.host_services;
        let parameters = &mut self.processing.input_parameters;
        parameters.service_output(&self.native.controller)?;
        services.service_edits(|id, value| parameters.queue_id_value(id, value).is_ok())?;

        let mut update = MainThreadUpdate::Unchanged;
        if requests.parameter_titles {
            self.processing
                .input_parameters
                .refresh_metadata(&self.native.controller)?;
            update = MainThreadUpdate::ParametersChanged;
        }
        // kParamValuesChanged invalidates host caches; it is not an outbound edit.
        // parameter_value() reads the controller directly, so there is no value cache
        // to refresh. Re-queuing restored values would block state capture until DSP runs.
        match latency_service_action(requests.latency) {
            LatencyServiceAction::Restart => self.restart_for_latency_change()?,
            LatencyServiceAction::Refresh => self.refresh_latency_snapshot(),
        }
        Ok(update)
    }
}
impl PluginCore for Vst3Plugin {
    fn info(&self) -> &PluginInfo {
        &self.info
    }
    fn active_layout(&self) -> Option<&BusLayout> {
        self.processing
            .active_layout
            .as_ref()
            .map(|layout| &layout.layout)
    }
    fn latency_frames(&self) -> Option<u32> {
        self.native
            .host_services
            .latency_snapshot_valid()
            .then_some(self.processing.latency_frames)
            .flatten()
    }
    fn supported_layouts(&self) -> &[BusLayout] {
        &self.layouts
    }

    fn parameter_count(&self) -> usize {
        self.param_count
    }

    fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        let info = self
            .processing
            .input_parameters
            .metadata(index)
            .ok_or(Error::InvalidParameter(index))?;
        Ok(vst3_param_info_to_rack(info))
    }

    fn parameter_value(&self, index: usize) -> Result<f64> {
        let info = self
            .processing
            .input_parameters
            .metadata(index)
            .ok_or(Error::InvalidParameter(index))?;
        Ok(unsafe { self.native.controller.getParamNormalized(info.id) })
    }

    fn parameter_value_string(&self, index: usize, _value: f64) -> Result<String> {
        // VST3 exposes IEditController::getParamStringByValue —
        // wiring it requires a TChar (UTF-16) buffer round-trip.
        // Tracked as a follow-on to avoid pulling in a wide-char
        // dep here.
        let _ = index;
        Err(Error::Other("vst3 parameter_value_string TODO".into()))
    }

    fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        self.native.host_services.ensure_creator_thread()?;
        self.processing
            .input_parameters
            .service_output(&self.native.controller)?;
        self.processing
            .input_parameters
            .set(&self.native.controller, index, value)?;
        self.native.host_services.invalidate_latency();
        Ok(())
    }

    fn has_pending_parameter_changes(&self) -> bool {
        self.processing.input_parameters.has_pending_changes()
            || self.native.host_services.has_pending_edits()
    }

    fn preset_count(&self) -> usize {
        // VST3 exposes presets via the `IUnitInfo` interface,
        // wired in a follow-on. Treat as zero for now.
        0
    }
    fn preset_info(&self, index: usize) -> Result<PresetInfo> {
        Err(Error::InvalidParameter(index))
    }
    fn load_preset(&mut self, _preset_number: i32) -> Result<()> {
        Err(Error::Other("vst3 preset loading not yet wired".into()))
    }

    fn save_state(&self) -> Result<Vec<u8>> {
        self.native.host_services.ensure_creator_thread()?;
        plugin_state::PluginState::capture(
            |stream| {
                // SAFETY: state capture runs on the fixed VST3 creator thread while the
                // component and its controller are alive; the stream is a live host COM object.
                unsafe { self.native.component.getState(stream) }
            },
            |stream| {
                // SAFETY: the controller is retained for the plugin lifetime and state capture
                // is serialized on its required host thread.
                unsafe { self.native.controller.getState(stream) }
            },
        )
        .map_err(|error| Error::Other(error.to_string()))
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<()> {
        self.native.host_services.ensure_creator_thread()?;
        plugin_state::PluginState::restore(
            bytes,
            || self.native.host_services.invalidate_latency(),
            |stream| {
                // SAFETY: restore validates the full container before this callback; the
                // component is retained and this method runs on its VST3 host thread.
                unsafe { self.native.component.setState(stream) }
            },
            |stream| {
                // SAFETY: this receives the same validated component section after component
                // restore, as required by the VST3 persistence call order.
                unsafe { self.native.controller.setComponentState(stream) }
            },
            |stream| {
                // SAFETY: this receives a validated controller section and runs on the fixed
                // creator thread while the controller remains retained.
                unsafe { self.native.controller.setState(stream) }
            },
        )
        .map_err(|error| Error::Other(error.to_string()))?;
        self.processing.input_parameters.discard_output();
        Ok(())
    }

    fn activate(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<()> {
        self.native.host_services.ensure_creator_thread()?;
        if self.is_active() {
            return Err(Error::Other(
                "VST3 must be deactivated before changing layout".into(),
            ));
        }
        let native_layout = bus_layout::select_layout(&self.processing.native_layouts, &layout)?;
        let prepared_output = self
            .processing
            .input_parameters
            .output_replacement(max_block_size)?;
        let maximum_block = i32::try_from(max_block_size)
            .map_err(|_| Error::Other("invalid VST3 maximum block size".into()))?;
        self.processing.latency_frames = None;
        let latency_revision = self.native.host_services.latency_revision();
        let mut setup = ProcessSetup {
            processMode: i32::from_ne_bytes(ProcessModes_::kRealtime.to_ne_bytes()),
            symbolicSampleSize: i32::from_ne_bytes(SymbolicSampleSizes_::kSample32.to_ne_bytes()),
            maxSamplesPerBlock: maximum_block,
            sampleRate: sample_rate,
        };
        activation::activate(
            &self.native.component,
            &self.native.processor,
            &native_layout,
            &mut setup,
            &mut self.processing.latency_frames,
        )?;
        self.processing
            .input_parameters
            .install_output(prepared_output);
        self.processing.processing = true;
        self.processing.active_layout = Some(native_layout);
        self.processing.activation_setup = Some(setup);
        self.publish_latency_revision_if_current(latency_revision);
        Ok(())
    }

    fn deactivate(&mut self) {
        if let Some(layout) = self.processing.active_layout.clone()
            && activation::deactivate(
                &self.native.component,
                &self.native.processor,
                &layout,
                &mut self.processing.processing,
            )
            .is_ok()
        {
            self.processing.active_layout = None;
            self.processing.activation_setup = None;
        }
        self.native.host_services.invalidate_latency();
    }
    fn is_active(&self) -> bool {
        self.processing.active_layout.is_some()
    }

    fn service_main_thread(&mut self) -> Result<MainThreadUpdate> {
        if let Err(error) = self.native.host_services.ensure_creator_thread() {
            self.processing.latency_frames = None;
            self.native.host_services.fail_closed();
            return Err(error);
        }
        let flags = self.native.host_services.take_restart_flags();
        if self.native.host_services.callbacks_failed() {
            self.processing.latency_frames = None;
            return Err(Error::Other("VST3 host callback previously failed".into()));
        }
        let result = self.service_requests(flags);
        if result.is_err() {
            self.processing.latency_frames = None;
            self.native.host_services.fail_closed();
        }
        result
    }

    fn editor(&mut self) -> Option<&mut dyn truce_rack_core::editor::PluginEditor> {
        if !self.info.has_editor {
            return None;
        }
        Some(self)
    }
}
