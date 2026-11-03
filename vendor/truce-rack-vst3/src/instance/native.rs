//! Own initialized COM interfaces through disconnect, termination, and final module exit.

use crate::{host_context, module::LoadedModule};
use vst3::{
    ComPtr,
    Steinberg::Vst::{
        IAudioProcessor, IComponent, IConnectionPoint, IConnectionPointTrait, IEditController,
    },
};

pub(super) struct NativeInstance {
    // Fields drop in declaration order: terminate, release interfaces, release callbacks, exit module.
    pub(super) _lifetimes: crate::lifecycle::PluginLifetimes,
    pub(super) controller_cp: Option<ComPtr<IConnectionPoint>>,
    pub(super) controller: ComPtr<IEditController>,
    pub(super) processor: ComPtr<IAudioProcessor>,
    pub(super) component_cp: Option<ComPtr<IConnectionPoint>>,
    pub(super) component: ComPtr<IComponent>,
    pub(super) host_services: host_context::HostServices,
    pub(super) _module: LoadedModule,
}

impl Drop for NativeInstance {
    fn drop(&mut self) {
        if let (Some(a), Some(b)) = (&self.component_cp, &self.controller_cp) {
            // SAFETY: both retained peers and their module outlive these synchronous calls.
            unsafe {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
        }
    }
}
