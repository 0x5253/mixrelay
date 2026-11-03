//! Guard successfully initialized VST3 objects until the adapter takes ownership.

use truce_rack_core::error::{Error, Result};
use vst3::{
    ComPtr,
    Steinberg::{FUnknown, IPluginBase, IPluginBaseTrait, kResultOk},
};

/// Calls `terminate` if a successfully initialized object exits a partially built load path.
pub(super) struct InitializedPlugin {
    plugin: ComPtr<IPluginBase>,
}

impl InitializedPlugin {
    pub(super) fn initialize(plugin: &ComPtr<IPluginBase>, context: *mut FUnknown) -> Result<Self> {
        // SAFETY: `plugin` is retained and context is owned by HostServices through termination.
        if unsafe { plugin.initialize(context) } != kResultOk {
            return Err(Error::Other("VST3 IPluginBase::initialize failed".into()));
        }
        Ok(Self {
            plugin: plugin.clone(),
        })
    }
}

impl Drop for InitializedPlugin {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after successful initialize and runs once before
        // the last retained interface/module owner is dropped on the creator thread.
        unsafe { self.plugin.terminate() };
    }
}

/// Terminate the separate controller before its component while all interfaces live.
pub(super) struct PluginLifetimes {
    _controller: Option<InitializedPlugin>,
    _component: InitializedPlugin,
}

impl PluginLifetimes {
    pub(super) fn new(component: InitializedPlugin, controller: Option<InitializedPlugin>) -> Self {
        Self {
            _controller: controller,
            _component: component,
        }
    }
}

#[cfg(test)]
mod tests;
