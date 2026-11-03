//! VST3 host implementation for the truce-rack framework.
//!
//! Built on the community `vst3` Rust bindings — no Steinberg SDK
//! submodule, no cmake. A fresh checkout builds in seconds.
//!
//! # Lifecycle
//!
//! Each `.vst3` bundle exports `GetPluginFactory` plus its platform
//! module entry and exit points. Loading walks:
//!
//! 1. `dlopen` the binary inside the bundle.
//! 2. The platform entry point (`bundleEntry`, `ModuleEntry`, or optional `InitDll`).
//! 3. `GetPluginFactory()` → `IPluginFactory`.
//! 4. `createInstance(cid, IComponent::IID)` → `IComponent`.
//! 5. `IPluginBase::initialize(host_context)` on the component.
//! 6. Cast to `IAudioProcessor`.
//! 7. `IComponent::getControllerClassId(&mut cid2)` — if separate,
//!    `factory.createInstance(cid2, IEditController::IID)` and
//!    initialize it. Otherwise cast the component itself.
//! 8. Connect the component and controller `IConnectionPoint`s
//!    (separate-controller case only).
//!
//! Activate calls `setBusArrangements`, `setupProcessing`,
//! `setActive(true)`, `setProcessing(true)`. Deactivate reverses.

mod activation;
mod bus_layout;
mod editor_frame;
mod event_exchange;
mod host_context;
mod identity;
mod instance;
mod latency;
mod lifecycle;
mod loading;
mod metadata;
mod module;
mod parameter_delivery;
mod plugin_state;
mod process_context;
mod scanner;
mod state_stream;
#[cfg(test)]
mod test_support;

pub use instance::Vst3Plugin;
pub use scanner::{Vst3Scanner, default_vst3_paths};

// Existing internal fixtures construct native parameter metadata through this path.
#[cfg(test)]
use metadata::empty_parameter_info;

/// Format identifier used on returned plug-in metadata.
pub const FORMAT: &str = "vst3";
/// Bundle directory suffix every VST3 plug-in uses.
pub const VST3_EXTENSION: &str = ".vst3";
