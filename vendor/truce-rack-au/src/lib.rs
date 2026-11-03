//! Audio Unit hosting with serialized control and preallocated planar rendering.
//!
//! [`AuScanner`] discovers registry components and loads [`AuPlugin`] with verified native bus
//! layouts. Render input planes are borrowed only during synchronous native rendering. Callback
//! allocations remain live until the native instance has been disposed.
//!
//! Control APIs check the creator thread; editor creation and teardown also require the macOS
//! main thread. Native plug-in code executes in the caller's process, so the host provides the
//! process boundary for crashes and calls that do not return.
//!
//! Components requiring asynchronous instantiation use a bounded run-loop wait. The synchronous
//! adapter still requires qualification for the host's actual `AUv3` lifecycle and editor.

#![cfg(target_vendor = "apple")]

mod activation;
mod au3_editor;
mod bus_topology;
mod channel_layouts;
mod control;
mod editor;
mod host_transport;
mod instantiation;
mod latency;
mod latency_signal;
mod parameters;
mod rendering;
mod scanner;
mod state;
mod stream_format;

use editor::AuEditorState;
use latency_signal::LatencySignalLease;
use objc2_audio_toolbox::{AudioComponentInstance, AudioUnitParameterID};
pub use scanner::AuScanner;
use std::thread::ThreadId;
use truce_rack_core::{bus::BusLayout, info::PluginInfo};

/// Format identifier used on returned [`PluginInfo`].
pub const FORMAT: &str = "au";

/// One loaded AU instance with serialized control access and owned render/editor state.
pub struct AuPlugin {
    info: PluginInfo,
    layouts: Vec<BusLayout>,
    input_bus_channels: Vec<u32>,
    output_bus_channels: Vec<u32>,
    active_layout: Option<BusLayout>,
    // Native initialization can outlive a failed activation or failed uninitialization.
    initialized: bool,
    /// Sample rate for the current activation, used only to convert the AU's seconds value.
    sample_rate: f64,
    /// Cached intrinsic latency; native property calls stay on the serialized lifecycle path.
    latency_frames: Option<u32>,
    /// Fixed-slot, generation-checked callback lease; it is released only after unit disposal.
    latency_signal: Option<LatencySignalLease<'static>>,
    /// All serialized AU control APIs remain bound to the instance's creation thread.
    creator_thread: ThreadId,
    instance: AudioComponentInstance,
    /// Parameter id list, cached at load. AU exposes parameters
    /// by `AudioUnitParameterID` (u32); truce-rack-core's trait surface
    /// uses zero-based indices. This vec is the index → id map.
    param_ids: Vec<AudioUnitParameterID>,
    render: rendering::RenderState,
    /// Live editor state, populated by `PluginEditor::open`. The
    /// view-factory object (`AUCocoaUIBase`) and the resulting
    /// `NSView` are kept alive here; dropping the plugin tears them
    /// down via objc2's automatic retain/release.
    editor: AuEditorState,
    /// `AUv3` views use a view-controller request instead of the `AUv2` Cocoa factory.
    is_v3: bool,
}

// SAFETY: active native rendering is serialized through exclusive access. Control calls
// additionally check the instance's creator thread before touching its native state.
unsafe impl Send for AuPlugin {}

#[cfg(test)]
mod tests;
