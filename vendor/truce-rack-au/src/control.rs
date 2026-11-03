//! Host control trait routing for one serialized Audio Unit instance.

use crate::AuPlugin;
use crate::latency::{
    latency_snapshot, query_latency_frames, take_latency_invalidation, with_latency_invalidation,
};
use crate::parameters::{au_parameter_to_rack, fetch_factory_presets, fetch_parameter_info};
use crate::stream_format::{
    activation_error, apply_and_verify_native_formats, capture_native_formats, set_maximum_frames,
};
use objc2_audio_toolbox::{
    AUPreset, AudioUnitGetParameter, AudioUnitSetParameter, AudioUnitSetProperty,
    AudioUnitUninitialize, kAudioUnitProperty_PresentPreset, kAudioUnitScope_Global,
};

use std::ptr;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::{ParameterInfo, PluginInfo, PresetInfo};
use truce_rack_core::plugin::{MainThreadUpdate, PluginCore};

impl PluginCore for AuPlugin {
    fn info(&self) -> &PluginInfo {
        &self.info
    }
    fn active_layout(&self) -> Option<&BusLayout> {
        self.active_layout.as_ref()
    }
    fn latency_frames(&self) -> Option<u32> {
        latency_snapshot(self.latency_frames, self.latency_signal.as_ref())
    }
    fn service_main_thread(&mut self) -> Result<truce_rack_core::plugin::MainThreadUpdate> {
        self.ensure_creator_thread()?;
        if take_latency_invalidation(self.latency_signal.as_ref()) {
            self.latency_frames = if self.is_active() && self.latency_signal.is_some() {
                // SAFETY: this service hook runs with exclusive access on the instance's
                // serialized owner thread, apart from the atomic-only property callback.
                unsafe { query_latency_frames(self.instance, self.sample_rate) }
            } else {
                None
            };
        }
        Ok(MainThreadUpdate::Unchanged)
    }
    fn supported_layouts(&self) -> &[BusLayout] {
        &self.layouts
    }
    fn parameter_count(&self) -> usize {
        self.param_ids.len()
    }

    fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
        self.ensure_creator_thread()?;
        let id = *self
            .param_ids
            .get(index)
            .ok_or(Error::InvalidParameter(index))?;
        let info = unsafe { fetch_parameter_info(self.instance, id)? };
        Ok(au_parameter_to_rack(id, &info))
    }

    fn parameter_value(&self, index: usize) -> Result<f64> {
        self.ensure_creator_thread()?;
        let id = *self
            .param_ids
            .get(index)
            .ok_or(Error::InvalidParameter(index))?;
        let mut value: f32 = 0.0;
        let status = unsafe {
            AudioUnitGetParameter(
                self.instance,
                id,
                kAudioUnitScope_Global,
                0,
                ptr::NonNull::new_unchecked(&raw mut value),
            )
        };
        if status != 0 {
            return Err(Error::Other(format!(
                "AudioUnitGetParameter failed: OSStatus {status}"
            )));
        }
        Ok(f64::from(value))
    }

    fn parameter_value_string(&self, _index: usize, _value: f64) -> Result<String> {
        self.ensure_creator_thread()?;
        // AU exposes a parameter-value-to-string property via
        // kAudioUnitProperty_ParameterValueStrings (for indexed
        // params) and AUParameterStringFromValue (for everything
        // else). Neither maps cleanly to the truce-rack-core surface
        // yet; tracked as a follow-on. For now, fall back to the
        // unit-stripped numeric form so callers get *something*
        // they can display.
        Err(Error::Other(
            "au parameter_value_string not yet wired".into(),
        ))
    }

    fn set_parameter(&mut self, index: usize, value: f64) -> Result<()> {
        self.ensure_creator_thread()?;
        let id = *self
            .param_ids
            .get(index)
            .ok_or(Error::InvalidParameter(index))?;
        #[allow(clippy::cast_possible_truncation)]
        let v = value as f32;
        let instance = self.instance;
        let status = with_latency_invalidation(
            &mut self.latency_frames,
            self.latency_signal.as_ref(),
            || unsafe { AudioUnitSetParameter(instance, id, kAudioUnitScope_Global, 0, v, 0) },
        );
        if status != 0 {
            return Err(Error::Other(format!(
                "AudioUnitSetParameter failed: OSStatus {status}"
            )));
        }
        Ok(())
    }
    fn preset_count(&self) -> usize {
        if self.ensure_creator_thread().is_err() {
            return 0;
        }
        unsafe { fetch_factory_presets(self.instance) }.map_or(0, |v| v.len())
    }

    fn preset_info(&self, index: usize) -> Result<PresetInfo> {
        self.ensure_creator_thread()?;
        let presets = unsafe { fetch_factory_presets(self.instance) }
            .ok_or_else(|| Error::Other("kAudioUnitProperty_FactoryPresets failed".into()))?;
        let preset = presets.get(index).ok_or(Error::InvalidParameter(index))?;
        Ok(preset.clone())
    }

    fn load_preset(&mut self, preset_number: i32) -> Result<()> {
        self.ensure_creator_thread()?;
        let preset = AUPreset {
            presetNumber: preset_number,
            presetName: ptr::null(),
        };
        let instance = self.instance;
        let status = with_latency_invalidation(
            &mut self.latency_frames,
            self.latency_signal.as_ref(),
            || unsafe {
                AudioUnitSetProperty(
                    instance,
                    kAudioUnitProperty_PresentPreset,
                    kAudioUnitScope_Global,
                    0,
                    (&raw const preset).cast(),
                    u32::try_from(std::mem::size_of::<AUPreset>()).unwrap_or(0),
                )
            },
        );
        if status != 0 {
            return Err(Error::Other(format!(
                "AudioUnitSetProperty(PresentPreset) failed: OSStatus {status}"
            )));
        }
        Ok(())
    }
    fn save_state(&self) -> Result<Vec<u8>> {
        self.ensure_creator_thread()?;
        // SAFETY: the creator-thread guard and exclusive host control serialize native access.
        unsafe { crate::state::capture(self.instance) }
    }

    fn load_state(&mut self, bytes: &[u8]) -> Result<()> {
        self.ensure_creator_thread()?;
        if self.initialized && !self.is_active() {
            return Err(Error::Other(
                "Audio Unit must finish uninitializing before state restoration".into(),
            ));
        }
        let plist = crate::state::decode(bytes)?;
        let instance = self.instance;
        with_latency_invalidation(
            &mut self.latency_frames,
            self.latency_signal.as_ref(),
            || {
                // SAFETY: the decoded state is retained, and this host owns serialized native access.
                unsafe { crate::state::restore(instance, &plist) }
            },
        )
    }
    fn activate(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<()> {
        self.ensure_creator_thread()?;
        let max_frames = self.validate_activation_request(&layout, sample_rate, max_block_size)?;
        self.invalidate_latency();
        self.sample_rate = 0.0;
        let previous_formats = unsafe {
            capture_native_formats(
                self.instance,
                &self.input_bus_channels,
                &self.output_bus_channels,
            )?
        };
        self.input_bus_channels = layout
            .inputs
            .iter()
            .map(|bus| bus.channels.count())
            .collect();
        self.output_bus_channels = layout
            .outputs
            .iter()
            .map(|bus| bus.channels.count())
            .collect();
        self.prepare_activation_storage(max_block_size)?;
        if let Err(cause) = unsafe {
            apply_and_verify_native_formats(
                self.instance,
                &self.input_bus_channels,
                &self.output_bus_channels,
                &layout,
                sample_rate,
            )
        } {
            return Err(unsafe {
                activation_error(self.instance, &previous_formats, Error::Other(cause))
            });
        }
        if let Err(cause) = unsafe { set_maximum_frames(self.instance, max_frames) } {
            return Err(unsafe {
                activation_error(self.instance, &previous_formats, Error::Other(cause))
            });
        }
        if let Err(cause) = unsafe { self.install_input_callbacks() } {
            return Err(unsafe {
                activation_error(self.instance, &previous_formats, Error::Other(cause))
            });
        }
        unsafe { self.install_host_callbacks() };
        self.initialize_selected_layout(&layout, sample_rate, &previous_formats)?;
        self.complete_activation(layout, sample_rate);
        Ok(())
    }
    fn deactivate(&mut self) {
        if self.ensure_creator_thread().is_err() {
            return;
        }
        if self.initialized {
            let status = unsafe { AudioUnitUninitialize(self.instance) };
            if status == 0 {
                self.initialized = false;
                self.render.suspend();
            }
        }
        self.active_layout = None;
        self.invalidate_latency();
        self.sample_rate = 0.0;
        self.render.max_block_size = 0;
    }
    fn is_active(&self) -> bool {
        self.initialized
    }

    fn editor(&mut self) -> Option<&mut dyn truce_rack_core::editor::PluginEditor> {
        if self.ensure_creator_thread().is_ok() && self.info.has_editor {
            Some(self)
        } else {
            None
        }
    }
}
