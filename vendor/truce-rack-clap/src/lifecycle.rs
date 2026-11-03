//! Activation, restart snapshots, and ordered instance destruction.

use crate::ClapPlugin;
use crate::audio_ports::AudioPortCatalog;
use crate::editor::{EditorState, close_gui_native, gui_supports_current_platform};
use crate::events::ConvertedInputEvents;
use crate::host::HostContext;
use crate::library::{EntryInitGuard, LoadedLibrary, PluginInstanceGuard, lookup_extension};
use crate::parameter_ids::ParameterIds;
use crate::parameters::{Parameters, read_parameter_ids};
use crate::process::ProcessChannelPointers;
use crate::scanner::bundle_binary_path;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::audio_ports::{CLAP_EXT_AUDIO_PORTS, clap_plugin_audio_ports};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, CLAP_EXT_AUDIO_PORTS_CONFIG_INFO, clap_plugin_audio_ports_config,
    clap_plugin_audio_ports_config_info,
};
use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_plugin_gui};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_plugin_params};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::ext::timer_support::{CLAP_EXT_TIMER_SUPPORT, clap_plugin_timer_support};
use clap_sys::plugin::clap_plugin;
use std::ffi::CString;
use std::ptr;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::info::PluginInfo;

#[derive(Default)]
pub(super) struct ProcessingState {
    pub(super) active_layout: Option<BusLayout>,
    pub(super) started_processing: bool,
    pub(super) activation: Option<ActivationConfig>,
    pub(super) completed_restart_generation: u64,
    pub(super) completed_latency_generation: u64,
    pub(super) process_pointers: Option<ProcessChannelPointers>,
    pub(super) input_events: Option<ConvertedInputEvents>,
    pub(super) latency_frames: Option<u32>,
    pub(super) steady_time: i64,
}

#[derive(Clone)]
pub(super) struct ActivationConfig {
    pub(super) layout: BusLayout,
    pub(super) sample_rate: f64,
    pub(super) max_block_size: usize,
}

pub(super) struct DiscoveredPluginMetadata {
    pub(super) info: PluginInfo,
    pub(super) audio_port_catalog: AudioPortCatalog,
    pub(super) parameter_ids: ParameterIds,
    pub(super) params_ext: *const clap_plugin_params,
    pub(super) state_ext: *const clap_plugin_state,
    pub(super) gui_ext: *const clap_plugin_gui,
    pub(super) timer_ext: *const clap_plugin_timer_support,
    pub(super) latency_ext: *const clap_plugin_latency,
}

pub(super) struct ActivationSnapshot {
    pub(super) parameter_ids: ParameterIds,
    pub(super) input_events: ConvertedInputEvents,
    pub(super) latency_frames: Option<u32>,
    pub(super) latency_generation: u64,
}

/// Native endpoints and stable host state for one activation/restart lifecycle operation.
#[derive(Clone, Copy)]
pub(super) struct ActivationTarget<'a> {
    pub(super) plugin: *const clap_plugin,
    pub(super) host_context: &'a HostContext,
    pub(super) params_ext: *const clap_plugin_params,
    pub(super) latency_ext: *const clap_plugin_latency,
}

impl ActivationTarget<'_> {
    pub(super) fn activate(&self, config: &ActivationConfig) -> Result<ActivationSnapshot> {
        let _main_thread = self.host_context.main_thread_scope()?;
        let max_block_size = u32::try_from(config.max_block_size)
            .map_err(|_| Error::Other("CLAP max block size exceeds u32".into()))?;
        let activate = unsafe { (*self.plugin).activate }
            .ok_or_else(|| Error::Other("clap plugin missing activate".into()))?;
        if !unsafe { activate(self.plugin, config.sample_rate, 1, max_block_size) } {
            return Err(Error::Other("clap_plugin::activate returned false".into()));
        }

        let refreshed = (|| {
            let parameter_ids = read_parameter_ids(self.plugin, self.params_ext)?;
            let input_events = ConvertedInputEvents::for_process(parameter_ids.len())?;
            // CLAP permits host.latency.changed while being activated. Query after activation and
            // parameter refresh, then absorb notifications through this snapshot.
            // SAFETY: the live plugin is active and this is its fixed main thread.
            let latency_frames = unsafe { query_latency_frames(self.plugin, self.latency_ext) };
            let latency_generation = self.host_context.latency_changed_generation();
            Ok::<_, Error>(ActivationSnapshot {
                parameter_ids,
                input_events,
                latency_frames,
                latency_generation,
            })
        })();
        match refreshed {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                if let Some(deactivate) = unsafe { (*self.plugin).deactivate } {
                    unsafe { deactivate(self.plugin) };
                }
                Err(error)
            }
        }
    }

    pub(super) fn activate_with_cache(
        &self,
        config: &ActivationConfig,
        latency_cache: &mut Option<u32>,
    ) -> Result<ActivationSnapshot> {
        *latency_cache = None;
        let snapshot = self.activate(config)?;
        *latency_cache = snapshot.latency_frames;
        Ok(snapshot)
    }
}

pub(super) fn deactivate_native(
    plugin: *const clap_plugin,
    host_context: &HostContext,
    started_processing: &mut bool,
    active: bool,
) -> Result<()> {
    let _main_thread = host_context.main_thread_scope()?;
    if *started_processing {
        let stop = unsafe { (*plugin).stop_processing }
            .ok_or_else(|| Error::Other("clap plugin missing stop_processing".into()))?;
        {
            // CLAP audio-thread is symbolic; the exclusive owner may stop this instance from
            // its fixed main OS thread while no DSP call can overlap.
            let _audio_thread = host_context.audio_thread_scope();
            unsafe { stop(plugin) };
        }
        *started_processing = false;
    }
    if active {
        let deactivate = unsafe { (*plugin).deactivate }
            .ok_or_else(|| Error::Other("clap plugin missing deactivate".into()))?;
        unsafe { deactivate(plugin) };
    }
    Ok(())
}

/// Owned references and state needed for ordered CLAP instance teardown.
pub(super) struct InstanceDestruction<'a> {
    pub(super) plugin: *const clap_plugin,
    pub(super) entry: *const clap_plugin_entry,
    pub(super) host_context: &'a HostContext,
    pub(super) gui_ext: *const clap_plugin_gui,
    pub(super) gui_open: &'a mut bool,
    pub(super) started_processing: &'a mut bool,
    pub(super) active: bool,
}

/// Destroy an instance and deinitialize its CLAP entry after closing/deactivating it.
/// # Safety
/// The plugin and entry pointers must be null or point to live records from the same library.
/// The caller must retain that library and the host context until this function returns.
pub(super) unsafe fn destroy_instance(instance: InstanceDestruction<'_>) {
    let InstanceDestruction {
        plugin,
        entry,
        host_context,
        gui_ext,
        gui_open,
        started_processing,
        active,
    } = instance;
    let _main_thread = host_context
        .main_thread_scope()
        .expect("CLAP plugin owner must destroy the instance on its fixed main thread");
    if !plugin.is_null() {
        let _ = close_gui_native(plugin, host_context, gui_ext, gui_open);
        let _ = deactivate_native(plugin, host_context, started_processing, active);
        if let Some(destroy) = unsafe { (*plugin).destroy } {
            // SAFETY: owner holds the plugin and library until this returns.
            unsafe { destroy(plugin) };
        }
    }
    host_context.clear_timers();
    if !entry.is_null()
        && let Some(deinit) = unsafe { (*entry).deinit }
    {
        // SAFETY: caller keeps the dynamic library and host context alive through this callback.
        unsafe { deinit() };
    }
}

impl ClapPlugin {
    pub(super) fn activate_config(
        &mut self,
        config: &ActivationConfig,
        generation: u64,
    ) -> Result<()> {
        let _main_thread = self.host_context.main_thread_scope()?;
        self.processing.process_pointers = None;
        self.processing.input_events = None;
        self.audio_port_catalog
            .select_and_validate(&config.layout)?;
        let process_pointers = ProcessChannelPointers::for_layout(&config.layout)?;
        let target = ActivationTarget {
            plugin: self.plugin,
            host_context: &self.host_context,
            params_ext: self.parameters.extension,
            latency_ext: self.latency_ext,
        };
        let snapshot = target.activate_with_cache(config, &mut self.processing.latency_frames);
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.processing.active_layout = None;
                self.processing.latency_frames = None;
                return Err(error);
            }
        };

        self.processing.active_layout = Some(config.layout.clone());
        self.parameters.ids = snapshot.parameter_ids;
        self.processing.input_events = Some(snapshot.input_events);
        self.processing.completed_restart_generation = generation;
        self.processing.completed_latency_generation = snapshot.latency_generation;
        self.processing.process_pointers = Some(process_pointers);
        Ok(())
    }
}

impl ClapPlugin {
    pub(super) fn deactivate_native(&mut self) -> Result<()> {
        self.processing.latency_frames = None;
        deactivate_native(
            self.plugin,
            &self.host_context,
            &mut self.processing.started_processing,
            self.processing.active_layout.is_some(),
        )?;
        self.processing.active_layout = None;
        self.processing.process_pointers = None;
        self.processing.input_events = None;
        Ok(())
    }

    pub(super) fn load_from(info: &PluginInfo) -> Result<Self> {
        let bundle_path = info.path.clone();
        let binary = bundle_binary_path(&bundle_path);
        let library = unsafe { LoadedLibrary::open(&binary)? };
        let host_context = HostContext::new();
        let _main_thread = host_context.main_thread_scope()?;
        let entry = library.entry()?;
        unsafe { entry.init(&binary)? };
        let mut entry_guard = EntryInitGuard::new(entry.entry);
        let factory = unsafe { entry.factory() };
        if factory.is_null() {
            return Err(Error::LoadFailed {
                path: bundle_path,
                reason: "plugin has no clap.plugin-factory".into(),
            });
        }
        let Some(create) = (unsafe { (*factory).create_plugin }) else {
            return Err(Error::LoadFailed {
                path: bundle_path,
                reason: "factory missing create_plugin".into(),
            });
        };
        let id_cstring = CString::new(info.unique_id.as_str()).map_err(|e| Error::LoadFailed {
            path: bundle_path.clone(),
            reason: format!("plugin id is not a valid C string: {e}"),
        })?;
        let mut plugin_guard = PluginInstanceGuard::new();
        let plugin = unsafe { create(factory, host_context.host_ptr(), id_cstring.as_ptr()) };
        plugin_guard.set(plugin);
        if plugin.is_null() {
            return Err(Error::LoadFailed {
                path: bundle_path,
                reason: "factory.create_plugin returned NULL".into(),
            });
        }
        let Some(init) = (unsafe { (*plugin).init }) else {
            return Err(Error::LoadFailed {
                path: bundle_path,
                reason: "plugin missing init".into(),
            });
        };
        if !unsafe { init(plugin) } {
            return Err(Error::LoadFailed {
                path: bundle_path,
                reason: "clap_plugin::init returned false".into(),
            });
        }
        let metadata = discover_plugin_metadata(info, plugin)?;
        let plugin = plugin_guard.take();
        entry_guard.disarm();
        Ok(Self::from_loaded_parts(
            metadata,
            plugin,
            host_context,
            library,
        ))
    }

    pub(super) fn from_loaded_parts(
        metadata: DiscoveredPluginMetadata,
        plugin: *const clap_plugin,
        host_context: Box<HostContext>,
        library: LoadedLibrary,
    ) -> Self {
        let DiscoveredPluginMetadata {
            info,
            audio_port_catalog,
            parameter_ids,
            params_ext,
            state_ext,
            gui_ext,
            timer_ext,
            latency_ext,
        } = metadata;
        Self {
            info,
            audio_port_catalog,
            plugin,
            host_context,
            library,
            processing: ProcessingState::default(),
            parameters: Parameters {
                ids: parameter_ids,
                changed_pending: false,
                extension: params_ext,
            },
            editor: EditorState {
                extension: gui_ext,
                open: false,
            },
            state_ext,
            timer_ext,
            latency_ext,
        }
    }
}

pub(super) fn discover_plugin_metadata(
    info: &PluginInfo,
    plugin: *const clap_plugin,
) -> Result<DiscoveredPluginMetadata> {
    let params_ext = unsafe { lookup_extension::<clap_plugin_params>(plugin, CLAP_EXT_PARAMS) };
    let audio_ports_ext =
        unsafe { lookup_extension::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS) };
    let audio_ports_config_ext = unsafe {
        lookup_extension::<clap_plugin_audio_ports_config>(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG)
    };
    let audio_ports_config_info_ext = unsafe {
        lookup_extension::<clap_plugin_audio_ports_config_info>(
            plugin,
            CLAP_EXT_AUDIO_PORTS_CONFIG_INFO,
        )
    };
    // SAFETY: extensions belong to this live instance; the caller is on its creator thread and
    // discovery runs before activation.
    let audio_port_catalog = unsafe {
        AudioPortCatalog::discover(
            plugin,
            audio_ports_ext,
            audio_ports_config_ext,
            audio_ports_config_info_ext,
        )
    }?;
    let state_ext = unsafe { lookup_extension::<clap_plugin_state>(plugin, CLAP_EXT_STATE) };
    let gui_ext = unsafe { lookup_extension::<clap_plugin_gui>(plugin, CLAP_EXT_GUI) };
    let timer_ext =
        unsafe { lookup_extension::<clap_plugin_timer_support>(plugin, CLAP_EXT_TIMER_SUPPORT) };
    let latency_ext = unsafe { lookup_extension::<clap_plugin_latency>(plugin, CLAP_EXT_LATENCY) };
    let parameter_ids = read_parameter_ids(plugin, params_ext)?;
    let mut plugin_info = info.clone();
    plugin_info.has_editor |=
        !gui_ext.is_null() && unsafe { gui_supports_current_platform(plugin, gui_ext) };

    Ok(DiscoveredPluginMetadata {
        info: plugin_info,
        audio_port_catalog,
        parameter_ids,
        params_ext,
        state_ext,
        gui_ext,
        timer_ext,
        latency_ext,
    })
}

/// Read a CLAP latency value while the plugin is active on its lifecycle thread.
///
/// CLAP defines the extension as optional; without it the host has no reported intrinsic
/// latency and uses zero. A present but incomplete extension is unknown, not zero.
///
/// # Safety
/// `plugin` and any non-null `latency_ext` must belong to the same live instance, and this must
/// be called on the CLAP main thread while the plugin is being activated or active.
pub(super) unsafe fn query_latency_frames(
    plugin: *const clap_plugin,
    latency_ext: *const clap_plugin_latency,
) -> Option<u32> {
    if latency_ext.is_null() {
        return Some(0);
    }
    // SAFETY: the extension pointer came from this live CLAP plugin instance and is only
    // queried during activation on the serialized lifecycle thread.
    let get = unsafe { (*latency_ext).get }?;
    // SAFETY: CLAP permits latency.get while being activated or active; caller invokes it only
    // after a successful activate call and keeps the plugin instance alive.
    Some(unsafe { get(plugin) })
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        let entry = self
            .library
            .entry()
            .map_or(ptr::null(), |entry| entry.entry);
        // SAFETY: this owner keeps host and library fields alive for the complete teardown call.
        unsafe {
            destroy_instance(InstanceDestruction {
                plugin: self.plugin,
                entry,
                host_context: &self.host_context,
                gui_ext: self.editor.extension,
                gui_open: &mut self.editor.open,
                started_processing: &mut self.processing.started_processing,
                active: self.processing.active_layout.is_some(),
            });
        }
        // `library` drops here, unloading the dylib.
    }
}
