//! Public plugin lifecycle and control trait adaptation.

use crate::ClapPlugin;
use crate::lifecycle::ActivationConfig;
use crate::parameters::{
    c_buf_to_string, clap_param_info_to_rack, empty_param_info, flush_pending_parameter_changes,
    queue_cached_parameter_change,
};
use std::ffi::c_char;
use std::time::Instant;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{ParameterInfo, PluginInfo, PresetInfo};
use truce_rack_core::plugin::{MainThreadUpdate, PluginCore};

impl PluginCore for ClapPlugin {
    fn info(&self) -> &PluginInfo {
        &self.info
    }

    fn active_layout(&self) -> Option<&BusLayout> {
        self.processing.active_layout.as_ref()
    }

    fn latency_frames(&self) -> Option<u32> {
        (self.host_context.restart_generation() == self.processing.completed_restart_generation
            && self.host_context.latency_changed_generation()
                == self.processing.completed_latency_generation)
            .then_some(self.processing.latency_frames)
            .flatten()
    }

    fn service_main_thread(&mut self) -> Result<MainThreadUpdate> {
        let _main_thread = self.host_context.main_thread_scope()?;
        // Consume before the native callback. A request raised from on_main_thread remains set
        // for the next broker tick rather than recursively dispatching without a bound.
        // SAFETY: the broker owns the instance mutex and invokes this on the fixed main thread.
        unsafe { self.host_context.service_callback(self.plugin)? };
        if !self.timer_ext.is_null() {
            self.host_context
                .service_timers(self.plugin, self.timer_ext, Instant::now());
        }

        let generation = self.host_context.restart_generation();
        let latency_generation = self.host_context.latency_changed_generation();
        let mut parameters_changed = std::mem::take(&mut self.parameters.changed_pending);
        if generation != self.processing.completed_restart_generation
            || latency_generation != self.processing.completed_latency_generation
        {
            let Some(config) = self.processing.activation.clone() else {
                return Ok(if parameters_changed {
                    MainThreadUpdate::ParametersChanged
                } else {
                    MainThreadUpdate::Unchanged
                });
            };

            // The broker holds exclusive instance ownership, so this is the drain point for any
            // in-flight process call. Pending ID-addressed events must not cross the replacement
            // parameter map.
            self.processing.latency_frames = None;
            self.parameters.ids.clear_all_pending();
            self.deactivate_native()?;
            self.activate_config(&config, generation)?;
            parameters_changed = true;
        }

        Ok(if parameters_changed {
            MainThreadUpdate::ParametersChanged
        } else {
            MainThreadUpdate::Unchanged
        })
    }

    fn has_pending_parameter_changes(&self) -> bool {
        self.parameters.ids.has_pending()
    }

    fn supported_layouts(&self) -> &[BusLayout] {
        self.audio_port_catalog.layouts()
    }

    fn parameter_count(&self) -> usize {
        self.parameters.ids.len()
    }

    fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        let _main_thread = self.host_context.main_thread_scope()?;
        self.parameters.ids.id(index)?;
        if self.parameters.extension.is_null() {
            return Err(Error::InvalidParameter(index));
        }
        let get_info = unsafe { (*self.parameters.extension).get_info }
            .ok_or(Error::InvalidParameter(index))?;
        let mut info = empty_param_info();
        let idx_u32 = u32::try_from(index).map_err(|_| Error::InvalidParameter(index))?;
        let ok = unsafe { get_info(self.plugin, idx_u32, &raw mut info) };
        if !ok {
            return Err(Error::InvalidParameter(index));
        }
        Ok(clap_param_info_to_rack(&info))
    }

    fn parameter_value(&self, index: usize) -> Result<f64> {
        let _main_thread = self.host_context.main_thread_scope()?;
        if self.parameters.extension.is_null() {
            return Err(Error::InvalidParameter(index));
        }
        let id = self.parameters.ids.id(index)?;
        let get_value = unsafe { (*self.parameters.extension).get_value }
            .ok_or(Error::InvalidParameter(index))?;
        let mut out = 0.0f64;
        let ok = unsafe { get_value(self.plugin, id, &raw mut out) };
        if !ok {
            return Err(Error::InvalidParameter(index));
        }
        Ok(out)
    }

    fn parameter_value_string(&self, index: usize, value: f64) -> Result<String> {
        let _main_thread = self.host_context.main_thread_scope()?;
        if self.parameters.extension.is_null() {
            return Err(Error::InvalidParameter(index));
        }
        let id = self.parameters.ids.id(index)?;
        let value_to_text = unsafe { (*self.parameters.extension).value_to_text }
            .ok_or(Error::InvalidParameter(index))?;
        // Match the CLAP ABI on signed- and unsigned-c_char targets alike.
        let mut buf = [0 as c_char; clap_sys::string_sizes::CLAP_NAME_SIZE];
        let buf_len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        let ok = unsafe { value_to_text(self.plugin, id, value, buf.as_mut_ptr(), buf_len) };
        if !ok {
            return Err(Error::InvalidParameter(index));
        }
        Ok(c_buf_to_string(&buf))
    }

    fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        let _main_thread = self.host_context.main_thread_scope()?;
        if !self.is_active() {
            return Err(Error::NotActivated);
        }
        if self.parameters.extension.is_null() {
            return Err(Error::InvalidParameter(index));
        }
        queue_cached_parameter_change(&mut self.parameters.ids, index, value)?;
        // Queue the change either way; process() drains the queue
        // every block, and a non-processing host can call flush()
        // to apply it immediately.
        if !self.processing.started_processing {
            let _ = flush_pending_parameter_changes(
                self.plugin,
                self.parameters.extension,
                &self.host_context,
                &mut self.parameters.ids,
            )?;
        }
        Ok(())
    }

    fn preset_count(&self) -> usize {
        0
    }

    fn preset_info(&self, index: usize) -> Result<PresetInfo> {
        Err(Error::InvalidParameter(index))
    }

    fn load_preset(&mut self, _preset_number: i32) -> Result<()> {
        Err(Error::Other("clap preset loading not yet wired".into()))
    }

    fn save_state(&self) -> Result<Vec<u8>> {
        self.save_native_state()
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<()> {
        self.load_native_state(bytes)
    }

    fn activate(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<()> {
        if self.processing.active_layout.is_some() {
            return Err(Error::Other(
                "CLAP activate requires the prior instance to be deactivated".into(),
            ));
        }
        let config = ActivationConfig {
            layout,
            sample_rate,
            max_block_size,
        };
        self.processing.latency_frames = None;
        let generation = self.host_context.restart_generation();
        self.activate_config(&config, generation)?;
        self.processing.activation = Some(config);
        self.parameters.changed_pending = true;
        Ok(())
    }

    fn deactivate(&mut self) {
        let Ok(_main_thread) = self.host_context.main_thread_scope() else {
            self.processing.latency_frames = None;
            return;
        };
        if self.deactivate_native().is_ok() {
            self.processing.activation = None;
            self.parameters.ids.clear_all_pending();
        }
    }

    fn is_active(&self) -> bool {
        self.processing.active_layout.is_some()
    }

    fn editor(&mut self) -> Option<&mut dyn truce_rack_core::editor::PluginEditor> {
        if self.editor.extension.is_null() {
            return None;
        }
        Some(self)
    }
}
