//! LV2 activation, state ordering, and native instance replacement.

use crate::atoms::midi_buffer_capacity;
use crate::ports::{LatencyPort, latency_snapshot};
use crate::{Lv2Plugin, state};
use truce_rack_core::{
    bus::BusLayout,
    editor::PluginEditor,
    error::{Error, Result},
    info::{ParameterInfo, PluginInfo, PresetInfo},
    plugin::PluginCore,
};

impl Lv2Plugin {
    /// Discard the current native instance, deactivating it only when the host still considers
    /// it active. This is also used after failed state restore because native restore may mutate
    /// before returning an error, so that instance cannot be safely reused.
    pub(super) fn free_instance(&mut self) {
        if !self.instance.is_null() {
            // SAFETY: `instance` is owned from the matching Lilv instantiate call. Its world and
            // features remain owned by `self`; deactivate is called only for an active instance.
            unsafe {
                if self.active_layout.is_some() {
                    lilv_sys::lilv_instance_deactivate(self.instance);
                }
                lilv_sys::lilv_instance_free(self.instance);
            }
            self.instance = std::ptr::null_mut();
        }
        self.sample_rate = 0.0;
        self.active_layout = None;
    }

    /// Retain the instance when its rate and declared buffer bound are unchanged. Otherwise
    /// replace it before activation and preserve native state rather than discarding a preset.
    pub(super) fn prepare_instance(
        &mut self,
        sample_rate: f64,
        maximum_block: usize,
    ) -> Result<()> {
        let needs_reinstantiate = configuration_requires_new_instance(
            !self.instance.is_null(),
            self.sample_rate,
            sample_rate,
            self.features.options.maximum_block(),
            maximum_block,
        );
        if !needs_reinstantiate {
            return Ok(());
        }
        self.drain_pending_controls();
        let retained_state = if self.instance.is_null() {
            None
        } else {
            Some(state::save(self)?)
        };
        self.close_editor();
        self.free_instance();
        self.features
            .options
            .configure(sample_rate, maximum_block)?;
        // SAFETY: the plugin belongs to the retained Lilv world and all feature pointers remain
        // valid in `self`; the returned instance is owned until `free_instance`.
        let instance = unsafe {
            lilv_sys::lilv_plugin_instantiate(
                self.plugin,
                sample_rate,
                self.features.feature_ptrs.as_ptr(),
            )
        };
        if instance.is_null() {
            return Err(Error::Other("lilv_plugin_instantiate returned NULL".into()));
        }
        if let Err(error) = self.bindings.parameters.set_sample_rate(sample_rate) {
            // SAFETY: the new instance has not been activated or published, and this is the
            // matching owning free before returning a failed activation to the host.
            unsafe { lilv_sys::lilv_instance_free(instance) };
            return Err(error);
        }
        self.instance = instance;
        self.sample_rate = sample_rate;
        if let Some(bytes) = retained_state
            && let Err(error) = self.restore_state(&bytes)
        {
            self.free_instance();
            return Err(error);
        }
        Ok(())
    }

    fn prepare_midi_buffers(&mut self, capacity: usize) -> Result<()> {
        self.atoms.prepare(capacity)
    }

    /// Run the part of activation that is shared by ordinary and stateful activation.
    pub(super) fn activate_prepared(&mut self, layout: BusLayout) {
        // SAFETY: prepare_instance established a non-null inactive instance at the requested
        // rate, and activation runs on the same serialized lifecycle thread.
        unsafe { lilv_sys::lilv_instance_activate(self.instance) };
        self.active_layout = Some(layout);
        self.invalidate_latency_after_state_change();
    }

    pub(super) fn update_latency_snapshot(&mut self) {
        let value = match self.bindings.latency_port {
            LatencyPort::NotReported | LatencyPort::Unknown => None,
            LatencyPort::OutputControl { offset } => {
                self.bindings.control_values.get(offset).copied()
            }
        };
        self.bindings.latency_frames = latency_snapshot(self.bindings.latency_port, value);
    }

    pub(super) fn invalidate_latency_after_state_change(&mut self) {
        self.bindings.latency_frames = match self.bindings.latency_port {
            LatencyPort::NotReported => Some(0),
            LatencyPort::OutputControl { .. } | LatencyPort::Unknown => None,
        };
    }
}

impl PluginCore for Lv2Plugin {
    fn info(&self) -> &PluginInfo {
        &self.info
    }
    fn active_layout(&self) -> Option<&BusLayout> {
        self.active_layout.as_ref()
    }
    fn latency_frames(&self) -> Option<u32> {
        self.bindings.latency_frames
    }
    fn supported_layouts(&self) -> &[BusLayout] {
        &self.layouts
    }
    fn parameter_count(&self) -> usize {
        self.bindings.parameters.len()
    }
    fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        self.bindings.parameters.info(index)
    }
    fn parameter_value(&self, index: usize) -> Result<f64> {
        let stored = self
            .bindings
            .parameters
            .value(index, &self.bindings.control_values)?;
        let offset = self.bindings.parameters.offset(index)?;
        Ok(self
            .ui
            .controller
            .pending
            .peek(offset)
            .map_or(stored, f64::from))
    }
    fn parameter_value_string(&self, index: usize, value: f64) -> Result<String> {
        self.bindings.parameters.formatted(index, value)
    }
    fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        self.drain_pending_controls();
        self.bindings
            .parameters
            .set(index, value, &mut self.bindings.control_values)
    }
    fn preset_count(&self) -> usize {
        0
    }
    fn preset_info(&self, index: usize) -> Result<PresetInfo> {
        Err(Error::InvalidParameter(index))
    }
    fn load_preset(&mut self, _preset_number: i32) -> Result<()> {
        Err(Error::Other("lv2 preset loading not yet wired".into()))
    }
    fn save_state(&self) -> Result<Vec<u8>> {
        state::save(self)
    }
    fn load_state(&mut self, bytes: &[u8]) -> Result<()> {
        self.restore_state(bytes)?;
        self.invalidate_latency_after_state_change();
        Ok(())
    }

    fn activate(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<()> {
        self.activate_with_state(layout, sample_rate, max_block_size, None)
    }

    fn activate_with_state(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
        state: Option<&[u8]>,
    ) -> Result<()> {
        let midi_buffer_capacity = validate_activation_request(
            self.is_active(),
            &self.layouts,
            &layout,
            sample_rate,
            max_block_size,
        )?;
        self.bindings.latency_frames = None;
        let mut activation = Lv2Activation {
            plugin: self,
            layout: Some(layout),
            sample_rate,
            maximum_block: max_block_size,
            midi_buffer_capacity,
        };
        restore_then_activate(&mut activation, state)
    }

    fn deactivate(&mut self) {
        if !self.instance.is_null() && self.active_layout.is_some() {
            // SAFETY: this is the retained instance created by Lilv and active_layout proves
            // activation succeeded; this call is serialized with processing and state access.
            unsafe { lilv_sys::lilv_instance_deactivate(self.instance) };
        }
        self.active_layout = None;
        self.bindings.latency_frames = None;
    }
    fn is_active(&self) -> bool {
        self.active_layout.is_some()
    }

    fn editor(&mut self) -> Option<&mut dyn PluginEditor> {
        if self.ui.bundle.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

trait ActivationActions {
    fn prepare_instance(&mut self) -> Result<()>;
    fn restore(&mut self, state: &[u8]) -> Result<()>;
    fn activate(&mut self) -> Result<()>;
    fn discard_after_restore_failure(&mut self);
}

struct Lv2Activation<'a> {
    plugin: &'a mut Lv2Plugin,
    layout: Option<BusLayout>,
    sample_rate: f64,
    maximum_block: usize,
    midi_buffer_capacity: usize,
}

impl ActivationActions for Lv2Activation<'_> {
    fn prepare_instance(&mut self) -> Result<()> {
        self.plugin
            .prepare_midi_buffers(self.midi_buffer_capacity)?;
        self.plugin
            .prepare_instance(self.sample_rate, self.maximum_block)
    }

    fn restore(&mut self, state: &[u8]) -> Result<()> {
        self.plugin.restore_state(state)?;
        self.plugin.invalidate_latency_after_state_change();
        Ok(())
    }

    fn activate(&mut self) -> Result<()> {
        let layout = self
            .layout
            .take()
            .ok_or_else(|| Error::Other("LV2 activation layout was already consumed".into()))?;
        self.plugin.activate_prepared(layout);
        Ok(())
    }

    fn discard_after_restore_failure(&mut self) {
        self.plugin.close_editor();
        self.plugin.free_instance();
        self.plugin.invalidate_latency_after_state_change();
    }
}

fn restore_then_activate(actions: &mut impl ActivationActions, state: Option<&[u8]>) -> Result<()> {
    actions.prepare_instance()?;
    if let Some(state) = state
        && let Err(failure) = actions.restore(state)
    {
        actions.discard_after_restore_failure();
        return Err(failure);
    }
    actions.activate()
}

fn configuration_requires_new_instance(
    instance_exists: bool,
    current_sample_rate: f64,
    requested_sample_rate: f64,
    current_maximum_block: usize,
    requested_maximum_block: usize,
) -> bool {
    !instance_exists
        || (current_sample_rate - requested_sample_rate).abs() > f64::EPSILON
        || current_maximum_block != requested_maximum_block
}

fn validate_activation_request(
    active: bool,
    supported_layouts: &[BusLayout],
    layout: &BusLayout,
    sample_rate: f64,
    max_block_size: usize,
) -> Result<usize> {
    if active {
        return Err(Error::Other(
            "stateful activation requires an inactive instance".into(),
        ));
    }
    if !supported_layouts.contains(layout) {
        return Err(Error::Other(
            "requested LV2 bus layout is not a supported native topology".into(),
        ));
    }
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(Error::Other("invalid LV2 sample rate".into()));
    }
    midi_buffer_capacity(max_block_size)
}

#[cfg(test)]
mod tests;
