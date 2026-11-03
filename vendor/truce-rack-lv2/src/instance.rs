//! Loaded LV2 instance assembly and ordered editor/native teardown.

use crate::{
    atoms::LV2_ATOM_SEQUENCE_URI,
    discovery::World,
    editor::{Controller, discover_ui},
    features::Features,
    state,
};
use std::{ffi::CStr, sync::Mutex};
use truce_rack_core::{bus::BusLayout, error::Result, info::PluginInfo};

/// One loaded LV2 plugin.
///
/// Owns its `World` (lilv data is referenced by pointer from the
/// instance), its features, its per-port metadata, and the per-port
/// scratch buffers connected on every block.
pub struct Lv2Plugin {
    pub(super) info: PluginInfo,
    pub(super) layouts: Vec<BusLayout>,
    pub(super) active_layout: Option<BusLayout>,
    // Lilv instance metadata stays alive until explicit native teardown in Drop.
    pub(super) _world: Box<World>,
    pub(super) plugin: *const lilv_sys::LilvPlugin,
    pub(super) instance: *mut lilv_sys::LilvInstance,
    pub(super) sample_rate: f64,
    pub(super) bindings: crate::ports::PortBindings,
    pub(super) atoms: crate::atoms::AtomBuffers,
    pub(super) features: Box<Features>,
    pub(super) asset_dir: Mutex<Option<tempfile::TempDir>>,
    pub(super) restored_assets: Option<tempfile::TempDir>,
    pub(super) ui: crate::editor::UiOwner,
}

// SAFETY: We hand the plugin between the audio and main threads
// behind an `Arc<Mutex<_>>` exactly like every other truce-rack format.
unsafe impl Send for Lv2Plugin {}

impl Lv2Plugin {
    pub(super) fn load_from(info: &PluginInfo) -> Result<Self> {
        let (world, plugin) = World::load_plugin(info)?;

        // All port metadata and scalar storage are derived from this retained world.
        let (bindings, layout) =
            unsafe { crate::ports::PortBindings::load(world.ptr, plugin, &info.path) }?;

        // Pre-map the URIs we'll need at process time.
        let features = Features::new();
        let midi_event_urid = features
            .urid_map
            .map_uri(unsafe { CStr::from_ptr(lilv_sys::LILV_URI_MIDI_EVENT.as_ptr().cast()) });
        let atom_sequence_urid = features
            .urid_map
            .map_uri(unsafe { CStr::from_ptr(LV2_ATOM_SEQUENCE_URI.as_ptr().cast()) });

        let ui_bundle = unsafe { discover_ui(world.ptr, plugin) };
        let mut info = info.clone();
        info.has_editor = ui_bundle.is_some();

        // The box fixes the callback context address before registering it with any native UI.
        let controller = Box::new(Controller::for_ports(
            &bindings.ports,
            &bindings.control_value_offset,
            bindings.control_values.len(),
        ));

        let atoms = crate::atoms::AtomBuffers::new(
            midi_event_urid,
            atom_sequence_urid,
            &features.urid_map,
            bindings
                .ports
                .iter()
                .filter(|port| {
                    !port.is_input && matches!(port.kind, crate::ports::PortKind::AtomSequence)
                })
                .map(|port| port.index),
        );

        Ok(Self {
            info,
            layouts: vec![layout],
            active_layout: None,
            _world: world,
            plugin,
            instance: std::ptr::null_mut(),
            sample_rate: 0.0,
            bindings,
            atoms,
            features,
            asset_dir: Mutex::new(None),
            restored_assets: None,
            ui: crate::editor::UiOwner::new(ui_bundle, controller),
        })
    }

    pub(super) fn drain_pending_controls(&mut self) {
        for (offset, value) in self.bindings.control_values.iter_mut().enumerate() {
            if let Some(pending) = self.ui.controller.pending.take(offset) {
                *value = pending;
            }
        }
    }

    pub(super) fn effective_control_value(&self, offset: usize) -> f32 {
        self.ui
            .controller
            .pending
            .peek(offset)
            .unwrap_or(self.bindings.control_values[offset])
    }

    pub(super) fn restore_state(&mut self, bytes: &[u8]) -> Result<()> {
        // A prior UI edit precedes this restore. A publish during restore remains pending.
        self.drain_pending_controls();
        state::restore(self, bytes)
    }
}

impl Drop for Lv2Plugin {
    fn drop(&mut self) {
        // Tear down the editor first so the UI's `cleanup` runs
        // while the audio instance is still alive (some UIs talk
        // to the instance via the `instance-access` extension).
        self.close_editor();
        self.free_instance();
        // Metadata, port buffers, features, and state assets drop after native cleanup.
    }
}
