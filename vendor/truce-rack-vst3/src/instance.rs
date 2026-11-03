//! Compose one loaded VST3 instance from native, processing, and editor owners.

use crate::loading::{
    ComponentParts, ControllerParts, LoadContext, load_component, load_controller,
};
use crate::{
    activation, bus_layout, event_exchange, host_context, identity::hex_to_tuid,
    module::LoadedModule, parameter_delivery,
};
use connections::connect_peers;
use editor::create_editor_view;
use truce_rack_core::{
    bus::BusLayout,
    error::{Error, Result},
    info::PluginInfo,
};
use vst3::Steinberg::Vst::IEditControllerTrait;

mod connections;
mod control;
mod editor;
mod native;
mod processing;

/// One VST3 plug-in facade with separate native, DSP, and editor owners.
///
/// Drop closes the view and stops DSP before releasing native COM interfaces.
pub struct Vst3Plugin {
    info: PluginInfo,
    layouts: Vec<BusLayout>,
    param_count: usize,
    processing: processing::ProcessingState,
    editor: editor::EditorState,
    native: native::NativeInstance,
}
impl Vst3Plugin {
    pub(crate) fn load_from(info: &PluginInfo) -> Result<Self> {
        let module = unsafe { LoadedModule::open(&info.path) }?;
        let factory = module.factory()?;
        let host_services = host_context::HostServices::new();
        let cid = hex_to_tuid(&info.unique_id).ok_or_else(|| Error::LoadFailed {
            path: info.path.clone(),
            reason: format!("could not parse VST3 unique_id {:?}", info.unique_id),
        })?;
        let context = LoadContext {
            info,
            factory: &factory,
            host_services: &host_services,
        };
        let ComponentParts {
            component,
            processor,
            lifecycle: component_lifecycle,
        } = load_component(&context, &cid)?;
        let ControllerParts {
            controller,
            separate,
            lifecycle: controller_lifecycle,
        } = load_controller(&context, &component)?;

        if !separate {
            host_services
                .attach_handler(&controller)
                .map_err(|error| Error::LoadFailed {
                    path: info.path.clone(),
                    reason: error.to_string(),
                })?;
        }

        // Optional VST3 peer interfaces are used only as a complete connected pair. The guard
        // disconnects them if any later load step fails.
        let peer_connections =
            connect_peers(&component, &controller, separate).map_err(|error| {
                Error::LoadFailed {
                    path: info.path.clone(),
                    reason: error.to_string(),
                }
            })?;

        let param_count_raw = unsafe { controller.getParameterCount() }.max(0);
        #[allow(clippy::cast_sign_loss)]
        let param_count = param_count_raw as usize;
        let input_parameters =
            parameter_delivery::InputParameters::from_controller(&controller, param_count)?;
        let native_layouts = bus_layout::discover(&component, &processor)?;
        let layouts = native_layouts
            .iter()
            .map(|layout| layout.layout.clone())
            .collect();

        let mut info = info.clone();
        // The editor exists if `createView("editor")` returns a
        // non-null view. We probe by creating + releasing it once
        // at load time. (Some plugins are slow to create the view;
        // a heavier-weight host would defer this to first open.)
        info.has_editor = unsafe { create_editor_view(&controller) }.is_some();

        let event_exchange = event_exchange::EventExchange::new()?;
        let (component_cp, controller_cp) = peer_connections.into_parts();
        let lifetimes =
            crate::lifecycle::PluginLifetimes::new(component_lifecycle, controller_lifecycle);
        Ok(Self {
            info,
            layouts,
            param_count,
            processing: processing::ProcessingState {
                native_layouts,
                active_layout: None,
                latency_frames: None,
                activation_setup: None,
                input_parameters,
                processing: false,
                event_exchange,
            },
            editor: editor::EditorState::default(),
            native: native::NativeInstance {
                component,
                processor,
                controller,
                _lifetimes: lifetimes,
                component_cp,
                controller_cp,
                host_services,
                _module: module,
            },
        })
    }
}
impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        truce_rack_core::PluginEditor::close(self);
        activation::deactivate_before_terminate(
            &self.native.component,
            &self.native.processor,
            self.processing.active_layout.as_ref(),
            &mut self.processing.processing,
        );
    }
}
