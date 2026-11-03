//! Creator-thread lifecycle, activation rollback and native instance disposal.

use crate::AuPlugin;
use crate::au3_editor;
use crate::channel_layouts;
use crate::editor::{AuEditorState, has_cocoa_ui};
use crate::host_transport::{
    host_beat_and_tempo, host_musical_time, host_transport_state, host_transport_state2,
};
use crate::instantiation::{instantiate_async, instantiate_sync};
use crate::latency::{latency_property_changed, query_latency_frames};
use crate::parameters::fetch_parameter_ids;
use crate::rendering::render_input_callback;
use crate::scanner::parse_unique_id;
use crate::stream_format::{NativeBusFormat, activation_error, verify_native_formats};
use objc2_audio_toolbox::{
    AURenderCallbackStruct, AudioComponentDescription, AudioComponentFindNext, AudioComponentFlags,
    AudioComponentInstanceDispose, AudioUnitInitialize,
    AudioUnitRemovePropertyListenerWithUserData, AudioUnitSetProperty, HostCallbackInfo,
    kAudioUnitProperty_HostCallbacks, kAudioUnitProperty_Latency,
    kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Global, kAudioUnitScope_Input,
};
use std::ptr;
use std::thread::ThreadId;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::PluginInfo;

pub(super) fn check_creator_thread(creator: ThreadId) -> Result<()> {
    if std::thread::current().id().eq(&creator) {
        Ok(())
    } else {
        Err(Error::Other(
            "Audio Unit control API called from a non-creator thread".into(),
        ))
    }
}

impl AuPlugin {
    pub(super) fn invalidate_latency(&mut self) {
        self.latency_frames = None;
        if let Some(signal) = self.latency_signal.as_ref() {
            signal.mark_dirty();
        }
    }

    pub(super) fn ensure_creator_thread(&self) -> Result<()> {
        check_creator_thread(self.creator_thread)
    }

    pub(super) fn validate_activation_request(
        &self,
        layout: &BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<u32> {
        if self.initialized {
            return Err(Error::Other(
                "Audio Unit must be deactivated before reconfiguration".into(),
            ));
        }
        if !self.layouts.contains(layout) {
            return Err(Error::Other(
                "requested Audio Unit bus layout was not verified by native discovery".into(),
            ));
        }
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err(Error::Other("invalid Audio Unit sample rate".into()));
        }
        u32::try_from(max_block_size)
            .ok()
            .filter(|frames| *frames > 0)
            .ok_or_else(|| Error::Other("invalid Audio Unit maximum block size".into()))
    }

    pub(super) fn prepare_activation_storage(&mut self, max_block_size: usize) -> Result<()> {
        self.render.prepare(
            &self.input_bus_channels,
            &self.output_bus_channels,
            max_block_size,
        )
    }

    /// Install input render callbacks after their stable boxed refcon has been prepared.
    ///
    /// # Safety
    /// The instance is live, inactive, and exclusively accessed by activation.
    pub(super) unsafe fn install_input_callbacks(&mut self) -> std::result::Result<(), String> {
        let render_ctx_ptr = self
            .render
            .render_ctx
            .as_deref_mut()
            .map_or(ptr::null_mut(), ptr::from_mut);
        if render_ctx_ptr.is_null() {
            return Err("Audio Unit render context missing".into());
        }
        for index in 0..self.input_bus_channels.len() {
            let element = u32::try_from(index)
                .map_err(|_| "Audio Unit input bus index overflow".to_owned())?;
            let callback_struct = AURenderCallbackStruct {
                inputProc: Some(render_input_callback),
                inputProcRefCon: render_ctx_ptr.cast(),
            };
            let status = unsafe {
                AudioUnitSetProperty(
                    self.instance,
                    kAudioUnitProperty_SetRenderCallback,
                    kAudioUnitScope_Input,
                    element,
                    (&raw const callback_struct).cast(),
                    u32::try_from(std::mem::size_of::<AURenderCallbackStruct>()).unwrap_or(0),
                )
            };
            if status != 0 {
                return Err(format!(
                    "AudioUnitSetProperty(SetRenderCallback, input element {element}) failed: OSStatus {status}"
                ));
            }
        }
        Ok(())
    }

    /// Install optional transport callbacks backed by the retained host snapshot.
    ///
    /// # Safety
    /// The instance and transport box are live and exclusively accessed by activation.
    pub(super) unsafe fn install_host_callbacks(&mut self) {
        let Some(host_transport) = self.render.host_transport.as_deref_mut() else {
            return;
        };
        let host_callbacks = HostCallbackInfo {
            hostUserData: ptr::from_mut(host_transport).cast(),
            beatAndTempoProc: Some(host_beat_and_tempo),
            musicalTimeLocationProc: Some(host_musical_time),
            transportStateProc: Some(host_transport_state),
            transportStateProc2: Some(host_transport_state2),
        };
        // Host transport callbacks are optional; preserve the historical best-effort behavior.
        let _ = unsafe {
            AudioUnitSetProperty(
                self.instance,
                kAudioUnitProperty_HostCallbacks,
                kAudioUnitScope_Global,
                0,
                (&raw const host_callbacks).cast(),
                u32::try_from(std::mem::size_of::<HostCallbackInfo>()).unwrap_or(0),
            )
        };
    }

    pub(super) fn complete_activation(&mut self, layout: BusLayout, sample_rate: f64) {
        // Initialization settles sample-rate-dependent parameter state.
        self.param_ids = unsafe { fetch_parameter_ids(self.instance) };
        self.render.sample_time = 0.0;
        self.sample_rate = sample_rate;
        if let Some(signal) = self.latency_signal.as_ref() {
            signal.take_invalidation();
        }
        self.latency_frames = self
            .latency_signal
            .as_ref()
            .and_then(|_| unsafe { query_latency_frames(self.instance, sample_rate) });
        self.active_layout = Some(layout);
    }

    pub(super) fn initialize_selected_layout(
        &mut self,
        layout: &BusLayout,
        sample_rate: f64,
        previous: &[NativeBusFormat],
    ) -> Result<()> {
        // SAFETY: activation owns the instance exclusively and retains both callback boxes.
        let status = unsafe { AudioUnitInitialize(self.instance) };
        if status != 0 {
            // A failed initializer may have acquired resources. Confirm teardown before
            // advertising a reusable instance or restoring its stream formats.
            self.initialized = true;
            self.deactivate();
            if self.initialized {
                return Err(Error::Other(format!(
                    "AudioUnitInitialize failed: OSStatus {status}; AudioUnitUninitialize failed"
                )));
            }
            return Err(unsafe {
                activation_error(
                    self.instance,
                    previous,
                    Error::ActivationRejected {
                        reason: format!("AudioUnitInitialize failed: OSStatus {status}"),
                    },
                )
            });
        }
        self.initialized = true;
        // Initialization may change element counts or formats; never publish the requested
        // layout as active until the native instance agrees with it.
        if let Err(cause) = unsafe { verify_native_formats(self.instance, layout, sample_rate) } {
            self.deactivate();
            if self.initialized {
                return Err(Error::Other(format!(
                    "{cause}; AudioUnitUninitialize failed; instance cannot be reactivated"
                )));
            }
            return Err(unsafe { activation_error(self.instance, previous, Error::Other(cause)) });
        }
        Ok(())
    }

    pub(super) fn load_from(info: &PluginInfo, selected: Option<&BusLayout>) -> Result<Self> {
        let mut desc = parse_unique_id(&info.unique_id).ok_or_else(|| Error::LoadFailed {
            path: info.path.clone(),
            reason: format!("could not parse AU unique_id {:?}", info.unique_id),
        })?;
        let component = unsafe {
            AudioComponentFindNext(ptr::null_mut(), ptr::NonNull::new_unchecked(&raw mut desc))
        };
        if component.is_null() {
            return Err(Error::LoadFailed {
                path: info.path.clone(),
                reason: format!("no AU matching {}", info.unique_id),
            });
        }

        // Re-query the component's actual flags so we can detect
        // RequiresAsyncInstantiation — `desc` only carries our scan
        // mask, not the component's own.
        let mut comp_desc = AudioComponentDescription {
            componentType: 0,
            componentSubType: 0,
            componentManufacturer: 0,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        let _ = unsafe {
            objc2_audio_toolbox::AudioComponentGetDescription(
                component,
                ptr::NonNull::new_unchecked(&raw mut comp_desc),
            )
        };
        let flags = AudioComponentFlags::from_bits_retain(comp_desc.componentFlags);
        let needs_async = flags.contains(AudioComponentFlags::RequiresAsyncInstantiation);

        let instance = if needs_async {
            instantiate_async(component, &info.path)?
        } else {
            instantiate_sync(component, &info.path)?
        };
        // SAFETY: the new instance is exclusively owned here; either path only accesses this
        // inactive instance and any failure disposes it before it can escape.
        let topology = unsafe {
            match selected {
                Some(layout) => channel_layouts::select(instance, layout),
                None => channel_layouts::discover(instance),
            }
        };
        let (layouts, input_bus_channels, output_bus_channels) = match topology {
            Ok(topology) => topology,
            Err(error) => {
                // SAFETY: the native instance is live and has not escaped this failed load.
                unsafe { AudioComponentInstanceDispose(instance) };
                return Err(error);
            }
        };
        let latency_signal = crate::latency::register_listener(instance);
        let param_ids = unsafe { fetch_parameter_ids(instance) };
        let is_v3 = flags.contains(AudioComponentFlags::IsV3AudioUnit);
        let has_editor = if is_v3 {
            unsafe { au3_editor::has_custom_view(component) }
        } else {
            unsafe { has_cocoa_ui(instance) }
        };
        let mut updated = info.clone();
        updated.has_editor = has_editor;
        Ok(Self {
            info: updated,
            layouts,
            input_bus_channels,
            output_bus_channels,
            active_layout: None,
            initialized: false,
            sample_rate: 0.0,
            latency_frames: None,
            latency_signal,
            creator_thread: std::thread::current().id(),
            instance,
            param_ids,
            render: crate::rendering::RenderState::default(),
            editor: AuEditorState::default(),
            is_v3,
        })
    }
}

impl Drop for AuPlugin {
    fn drop(&mut self) {
        if self.is_v3 && self.ensure_creator_thread().is_ok() {
            truce_rack_core::editor::PluginEditor::close(self);
        }
        if !self.instance.is_null() {
            if let Some(signal) = self.latency_signal.as_ref() {
                // SAFETY: the instance is live and this token is the exact registered refcon.
                unsafe {
                    AudioUnitRemovePropertyListenerWithUserData(
                        self.instance,
                        kAudioUnitProperty_Latency,
                        Some(latency_property_changed),
                        signal.user_data(),
                    );
                }
            }
            // The signal lease remains live through unit disposal, then its generation is
            // released by field drop. An in-flight callback afterward sees only its stale token.
            unsafe { AudioComponentInstanceDispose(self.instance) };
        }
    }
}

use truce_rack_core::plugin::PluginCore;
