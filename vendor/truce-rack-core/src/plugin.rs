//! The host-facing [`Plugin`] trait and its supporting
//! per-block types.
//!
//! Format wrappers (`truce-rack-clap`, `truce-rack-vst3`, `truce-rack-au`, …)
//! implement [`Plugin`] for their per-format instance type. Host
//! applications then hold a `Box<dyn Plugin<S>>` (or a generic
//! `<P: Plugin<S>>`) without caring which format produced it.

use crate::buffer::AudioBuffer;
use crate::bus::BusLayout;
use crate::error::Result;
use crate::events::EventList;
use crate::info::{ParameterInfo, PluginInfo, PresetInfo};
use crate::sample::Sample;
use crate::transport::TransportInfo;

/// Per-block context carrying host state into `process` and
/// returning per-block side-channel data from it.
///
/// Plugins write outbound events (MIDI thru, parameter touches)
/// into `output_events`; the wrapper drains them after the call
/// returns. Hosts that don't care about outbound events pass an
/// empty list and ignore whatever the plugin pushes.
pub struct ProcessContext<'a> {
    /// Sample rate active for this block. Plugins should
    /// recompute coefficients when this changes between blocks
    /// (rare but legal — host sample-rate change without a
    /// full deactivate / activate cycle).
    pub sample_rate: f64,
    /// Maximum frames the plugin was prepared for. The buffer's
    /// `num_frames()` may be less; never more.
    pub max_block_size: usize,
    /// Host transport snapshot for this block. `None` when the
    /// host doesn't expose transport (most CLAP hosts via the
    /// optional `clap.transport` extension only on hosts that
    /// support it).
    pub transport: Option<TransportInfo>,
    /// Outbound event sink the plugin pushes parameter touches /
    /// MIDI thru into. Cleared by the wrapper at the start of
    /// each block.
    pub output_events: &'a mut EventList,
}

/// Hint from the plugin about whether more output is coming.
///
/// Mirrors CLAP's `clap_process_status`. Hosts use the hint to
/// decide whether to keep calling `process` on an idle channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    /// Normal output — the plugin has more work in subsequent
    /// blocks regardless of input. Default for live processing.
    Continue,
    /// The plugin has no output and won't produce any until
    /// fresh input or events arrive. Host may skip `process`
    /// calls until then.
    Sleep,
    /// Tail-out — the plugin will keep producing audio for
    /// `tail_samples` more samples even with silent input
    /// (reverb, delay).
    Tail {
        /// Remaining tail length in samples.
        tail_samples: u32,
    },
    /// Hard error during processing. Wrapper logs and the host
    /// should treat the block's output as garbage (silence is a
    /// safer fallback for live audio).
    Error,
}

/// Metadata invalidation produced while servicing a plug-in on its fixed main thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MainThreadUpdate {
    /// The host's parameter index, identifier, and range snapshot remains valid.
    #[default]
    Unchanged,
    /// Rebuild the parameter snapshot before accepting another indexed parameter edit.
    ParametersChanged,
}

/// Core sample-precision-erased interface every plugin exposes.
///
/// Methods that don't touch audio samples are here so a host can
/// query metadata before deciding whether to instantiate as
/// `Plugin<f32>` or `Plugin<f64>`. Mirrors truce's
/// `PluginLogicCore` shape — the leaf [`Plugin<S>`] adds the
/// sample-typed `process`.
pub trait PluginCore: Send {
    /// Plugin metadata as the wrapper scanned it.
    fn info(&self) -> &PluginInfo;

    /// The bus layout currently active. `None` until
    /// [`PluginCore::activate`] picks one.
    fn active_layout(&self) -> Option<&BusLayout>;

    /// All bus layouts the plugin supports. The host picks one
    /// and passes it to `activate`. Returned by reference into
    /// internally-cached metadata; cheap to call repeatedly.
    fn supported_layouts(&self) -> &[BusLayout];

    /// Number of parameters this plugin exposes.
    fn parameter_count(&self) -> usize;

    /// Metadata for parameter at `index` (0-based into the
    /// plugin's declared list).
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidParameter`] when `index >=
    /// parameter_count()`.
    fn parameter_info(&self, index: usize) -> Result<ParameterInfo>;

    /// Current value of parameter `index` in its native unit.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidParameter`] when out of
    /// range or [`crate::Error::NotActivated`] when called before
    /// `activate`.
    fn parameter_value(&self, index: usize) -> Result<f64>;

    /// Format the parameter value at `index` as the plugin
    /// would render it in its own UI. Many formats supply this
    /// directly (`clap_param_info_value_to_text`); others
    /// require host-side formatting.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidParameter`] when `index` is
    /// out of range or [`crate::Error::NotActivated`] when called
    /// before `activate`.
    fn parameter_value_string(&self, index: usize, value: f64) -> Result<String>;

    /// Set parameter `index` to `value` in native units on the same OS thread that created
    /// the instance, with exclusive access against processing, state, and editor calls.
    /// This host-thread setter may call native controllers and allocate; it is not a
    /// real-time callback. The adapter queues audio-thread events for subsequent real
    /// processing blocks, and the plugin may smooth toward the new value over those blocks.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidParameter`] when out of
    /// range or [`crate::Error::NotActivated`] when called before
    /// `activate`.
    fn set_parameter(&mut self, index: usize, value: f64) -> Result<()>;

    /// Whether accepted host parameter changes still await delivery to the native plug-in.
    ///
    /// This is a cached, nonallocating read with no native calls or locks. Queueing an event
    /// is not application: adapters keep this true until a native flush or successful real
    /// processing block consumes the changes. Hosts must defer native state capture while
    /// true rather than save a stale blob or synthesize audio for an OFF node. Adapters that
    /// apply setters synchronously may retain the default `false`.
    fn has_pending_parameter_changes(&self) -> bool {
        false
    }

    /// Number of factory presets, if the plugin exposes any.
    fn preset_count(&self) -> usize;

    /// Metadata for preset at `index`.
    ///
    /// # Errors
    /// Returns [`crate::Error::InvalidParameter`] when out of
    /// range.
    fn preset_info(&self, index: usize) -> Result<PresetInfo>;

    /// Load preset by the format-specific id from
    /// [`PresetInfo::preset_number`].
    ///
    /// # Errors
    /// Wrapper-specific — typically when the id is unknown.
    fn load_preset(&mut self, preset_number: i32) -> Result<()>;

    /// Snapshot plugin state to a byte blob. Wrap in
    /// [`crate::StateEnvelope`] before persisting if the host
    /// wants the version / format header.
    ///
    /// # Errors
    /// Wrapper-specific.
    fn save_state(&self) -> Result<Vec<u8>>;

    /// Restore plugin state from bytes previously returned by
    /// [`PluginCore::save_state`]. The host strips its own
    /// envelope before calling this — the bytes here are
    /// plugin-opaque.
    ///
    /// # Errors
    /// Wrapper-specific.
    fn load_state(&mut self, bytes: &[u8]) -> Result<()>;

    /// Pick a bus layout and prepare the plugin for processing
    /// at `sample_rate` with blocks up to `max_block_size`
    /// frames.
    ///
    /// Hosts must call `activate` before any `process` call.
    /// Subsequent reconfiguration (sample-rate change, layout
    /// switch) requires a `deactivate` + `activate` cycle.
    ///
    /// # Errors
    /// Wrapper-specific — typically when the requested layout
    /// isn't in [`PluginCore::supported_layouts`].
    fn activate(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
    ) -> Result<()>;

    /// Restore optional native state before negotiating `layout` and starting processing.
    ///
    /// Call only on an inactive instance, on its owning lifecycle thread. `state` contains
    /// the native bytes returned by [`PluginCore::save_state`], without a host envelope.
    /// Layout negotiation must run after restoration because state may change native ports.
    /// Adapters that instantiate at a particular sample rate override this method to create
    /// their native instance first, then restore state before activation or any audio run.
    ///
    /// # Errors
    /// Returns an error for an already-active instance, invalid state, or failed activation.
    /// A restoration failure must not start processing.
    fn activate_with_state(
        &mut self,
        layout: BusLayout,
        sample_rate: f64,
        max_block_size: usize,
        state: Option<&[u8]>,
    ) -> Result<()> {
        if self.is_active() {
            return Err(crate::Error::Other(
                "stateful activation requires an inactive instance".into(),
            ));
        }
        if let Some(state) = state {
            self.load_state(state)?;
        }
        self.activate(layout, sample_rate, max_block_size)
    }

    /// Tear down the active processing config. After this call
    /// the plugin holds no per-activation resources and `process`
    /// won't be called until the next `activate`.
    fn deactivate(&mut self);

    /// `true` when [`PluginCore::activate`] has been called and
    /// [`PluginCore::deactivate`] hasn't been called since.
    fn is_active(&self) -> bool;

    /// Returns the cached intrinsic latency in sample frames when known.
    ///
    /// This value excludes latency added by the host's worker transport. `Some(0)` means
    /// the format reports zero latency; `None` means no reliable value is available. This is
    /// a cheap snapshot read: implementations must not call plug-in APIs, allocate, or lock
    /// here. Format wrappers refresh native values only on a lifecycle thread allowed by that
    /// format. An LV2 adapter may copy the already-connected latency output scalar into its
    /// snapshot immediately after `run`; this does not call a plug-in API or allocate.
    fn latency_frames(&self) -> Option<u32> {
        None
    }

    /// Service coalesced native callbacks and lifecycle requests on the plug-in's main thread.
    ///
    /// Call on the same OS thread that created the instance, with exclusive access against
    /// processing, state, and editor calls. Implementations may allocate and perform native
    /// lifecycle operations here; never invoke this method from a real-time callback. One call
    /// must service bounded work, leaving requests raised during that work for a later call.
    /// Cached latency must remain unknown while a latency-changing restart is pending or failed.
    /// Adapters without host-service requests return [`MainThreadUpdate::Unchanged`].
    ///
    /// # Returns
    /// Indicates whether the host must replace its cached parameter identifiers and ranges.
    ///
    /// # Errors
    /// Returns a native callback, reactivation, or metadata-refresh failure. The host must stop
    /// processing the failed instance rather than publish old state or assume restart succeeded.
    /// The adapter must retain the initialized instance and editor ownership for orderly
    /// main-thread editor close, deactivation (when still active), and destruction; an error
    /// must not destroy the native object behind the host's back.
    fn service_main_thread(&mut self) -> Result<MainThreadUpdate> {
        Ok(MainThreadUpdate::Unchanged)
    }

    /// Borrow the plugin's editor controller if the plugin
    /// exposes a custom GUI. Returns `None` for headless plugins
    /// or plugins whose editor extension is missing.
    ///
    /// The returned reference borrows `&mut self`, which means the
    /// host can't call `process` (which also needs `&mut self`)
    /// while holding it. That's the Rust-level enforcement of the
    /// "audio thread vs UI thread" discipline.
    fn editor(&mut self) -> Option<&mut dyn crate::editor::PluginEditor> {
        None
    }
}

/// Sample-precision-typed leaf trait. Pairs `PluginCore` with the
/// `process` callback at a specific sample type.
///
/// Most format wrappers implement `Plugin<f32>`; ones that
/// support host-chosen 64-bit (VST3, AU v2/v3, AAX) implement
/// both `Plugin<f32>` and `Plugin<f64>` on the same instance
/// type or on a precision-specialised wrapper.
pub trait Plugin<S: Sample>: PluginCore {
    /// Process one audio block.
    ///
    /// # Real-time-safety contract
    ///
    /// This callback runs on the host's audio thread. The
    /// wrapper guarantees no allocator-touching work inside this
    /// crate on the call edge; the *plugin* code itself is
    /// expected to honor the same: no `Box::new`, no
    /// `Vec::push` past pre-grown capacity, no mutex locking,
    /// no I/O, no `panic!`. A panic is caught by the wrapper
    /// (see [`crate::wrapper::run_audio_block_with`]) and turned
    /// into [`ProcessStatus::Error`] but the host's block is
    /// still lost.
    ///
    /// # Errors
    /// Returns [`crate::Error::NotActivated`] when the plugin
    /// hasn't been activated; wrapper-specific errors otherwise.
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, S>,
        events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> Result<ProcessStatus>;
}
