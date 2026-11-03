//! A loadable CLAP fixture for native parameter, latency, and audio-bus qualification.

mod large_state;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value,
    clap_input_events,
};
use clap_sys::ext::audio_ports::{
    CLAP_AUDIO_PORT_IS_MAIN, CLAP_PORT_MONO, CLAP_PORT_STEREO, clap_audio_port_info,
    clap_plugin_audio_ports,
};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, CLAP_EXT_AUDIO_PORTS_CONFIG_INFO, clap_audio_ports_config,
    clap_plugin_audio_ports_config, clap_plugin_audio_ports_config_info,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_STEPPED, clap_param_info,
    clap_plugin_params,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::plugin_features::CLAP_PLUGIN_FEATURE_AUDIO_EFFECT;
use clap_sys::process::{CLAP_PROCESS_CONTINUE, CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::string_sizes::{CLAP_NAME_SIZE, CLAP_PATH_SIZE};
use clap_sys::version::CLAP_VERSION;
use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

const PLUGIN_ID: &CStr = c"io.mixrelay.test.native-clap-parameters-latency";
const PARAM_GAIN_A: clap_id = 1;
const PARAM_GAIN_B: clap_id = 2;
const PARAM_DELAY_FRAMES: clap_id = 3;
const CONFIG_STEREO: clap_id = 1;
const CONFIG_STEREO_SIDECHAIN: clap_id = 2;
const CONFIG_MONO: clap_id = 3;
const MAX_DELAY_FRAMES: u32 = 256;
const MAX_DELAY_FRAMES_USIZE: usize = 256;
const MAX_DELAY_FRAMES_F64: f64 = 256.0;
const STATE_MAGIC: [u8; 8] = *b"MRCLAP01";
const STATE_SIZE: usize = 28;
const LARGE_STATE_ENV: &str = "MIXRELAY_CLAP_FIXTURE_LARGE_STATE";

const PARAMETER_SPECS: [ParameterSpec; 3] = [
    ParameterSpec {
        id: PARAM_GAIN_A,
        name: b"Gain A",
        flags: CLAP_PARAM_IS_AUTOMATABLE,
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
    ParameterSpec {
        id: PARAM_GAIN_B,
        name: b"Gain B",
        flags: CLAP_PARAM_IS_AUTOMATABLE,
        min: 0.0,
        max: 1.0,
        default: 1.0,
    },
    ParameterSpec {
        id: PARAM_DELAY_FRAMES,
        name: b"Delay Frames",
        flags: CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_STEPPED,
        min: 0.0,
        max: MAX_DELAY_FRAMES_F64,
        default: 0.0,
    },
];

struct ParameterSpec {
    id: clap_id,
    name: &'static [u8],
    flags: u32,
    min: f64,
    max: f64,
    default: f64,
}

#[derive(Clone, Copy, Debug)]
struct ParameterValues {
    gain_a: f64,
    gain_b: f64,
    delay_frames: u32,
}

struct PluginState {
    host: *const clap_host,
    host_latency: AtomicPtr<clap_host_latency>,
    gain_a_bits: AtomicU64,
    gain_b_bits: AtomicU64,
    requested_delay_frames: AtomicU32,
    active_delay_frames: AtomicU32,
    current_config_id: AtomicU32,
    active: AtomicBool,
    large_state: bool,
    ring: UnsafeCell<DelayRing>,
}

struct ProcessBlock {
    input_channels: [*mut f32; 2],
    sidechain_channels: [*mut f32; 2],
    output_channels: [*mut f32; 2],
    channel_count: usize,
    has_sidechain: bool,
    frame_count: usize,
    events: *const clap_input_events,
    delay_frames: usize,
}

struct ProcessChannels {
    input: [*mut f32; 2],
    sidechain: [*mut f32; 2],
    output: [*mut f32; 2],
    channel_count: usize,
    has_sidechain: bool,
}

/// The fixed stereo history is touched only by `process` and serialized lifecycle callbacks.
/// CLAP requires activation, deactivation, reset, and audio processing for one instance not to
/// overlap. Parameter and latency state shared with the main thread stays in atomics.
struct DelayRing {
    frames: [[f32; 2]; MAX_DELAY_FRAMES_USIZE],
    cursor: usize,
}

impl PluginState {
    fn new(host: *const clap_host, large_state: bool) -> Self {
        Self {
            host,
            host_latency: AtomicPtr::new(ptr::null_mut()),
            gain_a_bits: AtomicU64::new(1.0_f64.to_bits()),
            gain_b_bits: AtomicU64::new(1.0_f64.to_bits()),
            requested_delay_frames: AtomicU32::new(0),
            active_delay_frames: AtomicU32::new(0),
            current_config_id: AtomicU32::new(CONFIG_STEREO),
            active: AtomicBool::new(false),
            large_state,
            ring: UnsafeCell::new(DelayRing::new()),
        }
    }

    fn parameter_values(&self) -> ParameterValues {
        ParameterValues {
            gain_a: f64::from_bits(self.gain_a_bits.load(Ordering::Acquire)),
            gain_b: f64::from_bits(self.gain_b_bits.load(Ordering::Acquire)),
            delay_frames: self.requested_delay_frames.load(Ordering::Acquire),
        }
    }
}

impl DelayRing {
    fn new() -> Self {
        Self {
            frames: [[0.0; 2]; MAX_DELAY_FRAMES_USIZE],
            cursor: 0,
        }
    }

    fn reset(&mut self) {
        self.frames.fill([0.0; 2]);
        self.cursor = 0;
    }

    fn process_frame(&mut self, input: [f32; 2], delay_frames: usize) -> [f32; 2] {
        if delay_frames == 0 {
            return input;
        }

        let output = self.frames[self.cursor];
        self.frames[self.cursor] = input;
        self.cursor += 1;
        if self.cursor == delay_frames {
            self.cursor = 0;
        }
        output
    }
}

impl ParameterValues {
    fn encode(self) -> [u8; STATE_SIZE] {
        let mut bytes = [0_u8; STATE_SIZE];
        bytes[..STATE_MAGIC.len()].copy_from_slice(&STATE_MAGIC);
        bytes[8..16].copy_from_slice(&self.gain_a.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.gain_b.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.delay_frames.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8; STATE_SIZE]) -> Option<Self> {
        if bytes[..STATE_MAGIC.len()] != STATE_MAGIC {
            return None;
        }
        let gain_a = sanitize_gain(f64::from_le_bytes(bytes[8..16].try_into().ok()?));
        let gain_b = sanitize_gain(f64::from_le_bytes(bytes[16..24].try_into().ok()?));
        let delay_frames = u32::from_le_bytes(bytes[24..28].try_into().ok()?);
        if delay_frames > MAX_DELAY_FRAMES {
            return None;
        }
        Some(Self {
            gain_a,
            gain_b,
            delay_frames,
        })
    }
}

fn sanitize_gain(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_delay(value: f64) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let bounded = value.round().clamp(0.0, MAX_DELAY_FRAMES_F64);
    // The value is finite, rounded, and bounded to 0..=256 above.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let delay_frames = bounded as u32;
    delay_frames
}

fn name_buffer<const SIZE: usize>(name: &[u8]) -> [c_char; SIZE] {
    let mut buffer = [0; SIZE];
    for (destination, source) in buffer.iter_mut().zip(name.iter().copied()) {
        *destination = c_char::from_ne_bytes([source]);
    }
    buffer
}

unsafe fn plugin_state<'a>(plugin: *const clap_plugin) -> Option<&'a PluginState> {
    // SAFETY: CLAP callbacks pass the live plugin object originally returned by this factory.
    let plugin = unsafe { plugin.as_ref() }?;
    let state = plugin.plugin_data.cast::<PluginState>();
    // SAFETY: create_plugin stores a Box<PluginState> in plugin_data until destroy.
    unsafe { state.as_ref() }
}

unsafe fn input_event_count(events: *const clap_input_events) -> u32 {
    // SAFETY: CLAP passes either null or a live event-list interface for the current call.
    let Some(events) = (unsafe { events.as_ref() }) else {
        return 0;
    };
    let Some(size) = events.size else {
        return 0;
    };
    // SAFETY: the callback belongs to the supplied live CLAP input-event list.
    unsafe { size(events) }
}

unsafe fn input_event(
    events: *const clap_input_events,
    index: u32,
) -> Option<*const clap_event_header> {
    // SAFETY: CLAP passes a live event-list interface while process/flush runs.
    let events = unsafe { events.as_ref() }?;
    let get = events.get?;
    // SAFETY: caller bounds index by the count returned from this same event list.
    let header = unsafe { get(events, index) };
    (!header.is_null()).then_some(header)
}

fn set_parameter(state: &PluginState, parameter_id: clap_id, value: f64) {
    match parameter_id {
        PARAM_GAIN_A => state
            .gain_a_bits
            .store(sanitize_gain(value).to_bits(), Ordering::Release),
        PARAM_GAIN_B => state
            .gain_b_bits
            .store(sanitize_gain(value).to_bits(), Ordering::Release),
        PARAM_DELAY_FRAMES => {
            let delay_frames = sanitize_delay(value);
            let previous = state
                .requested_delay_frames
                .swap(delay_frames, Ordering::AcqRel);
            if previous != delay_frames && state.active.load(Ordering::Acquire) {
                // SAFETY: this state was recovered from a live plugin callback; its host pointer
                // remains owned by the host for the plugin instance's lifetime.
                unsafe { request_host_restart(state) };
            }
        }
        _ => {}
    }
}

unsafe fn apply_parameter_event(state: &PluginState, header: *const clap_event_header) {
    // SAFETY: callers validated the event type and complete size. read_unaligned handles CLAP
    // event storage whose alignment is only guaranteed for clap_event_header.
    let event = unsafe { header.cast::<clap_event_param_value>().read_unaligned() };
    set_parameter(state, event.param_id, event.value);
}

unsafe fn flush_parameter_events(events: *const clap_input_events, state: &PluginState) {
    let event_count = unsafe { input_event_count(events) };
    for index in 0..event_count {
        let Some(header) = (unsafe { input_event(events, index) }) else {
            continue;
        };
        // SAFETY: header points to an event object owned by the input list for this call.
        let header_ref = unsafe { &*header };
        if header_ref.space_id != CLAP_CORE_EVENT_SPACE_ID
            || header_ref.type_ != CLAP_EVENT_PARAM_VALUE
            || usize::try_from(header_ref.size).unwrap_or(0)
                < std::mem::size_of::<clap_event_param_value>()
        {
            continue;
        }
        // SAFETY: the header type and declared size match clap_event_param_value.
        unsafe { apply_parameter_event(state, header) };
    }
}

/// Request lifecycle reactivation from the host for a changed active delay.
///
/// # Safety
/// `state` must belong to a live CLAP instance, and its retained host pointer must remain valid.
unsafe fn request_host_restart(state: &PluginState) {
    let host = state.host;
    // SAFETY: host is retained from create_plugin and outlives the plugin instance.
    let Some(host_ref) = (unsafe { host.as_ref() }) else {
        return;
    };
    let Some(request_restart) = host_ref.request_restart else {
        return;
    };
    // SAFETY: this is the host callback paired with the retained host pointer.
    unsafe { request_restart(host) };
}

unsafe fn notify_host_latency_changed(state: &PluginState) {
    let latency = state.host_latency.load(Ordering::Acquire);
    // SAFETY: the host extension pointer was queried from this live host during plugin init.
    let Some(latency) = (unsafe { latency.as_ref() }) else {
        return;
    };
    let Some(changed) = latency.changed else {
        return;
    };
    // SAFETY: CLAP permits latency.changed on the main thread during activation.
    unsafe { changed(state.host) };
}

unsafe extern "C" fn entry_init(_plugin_path: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    if factory_id.is_null() {
        return ptr::null();
    }
    // SAFETY: CLAP passes a NUL-terminated factory identifier.
    let requested = unsafe { CStr::from_ptr(factory_id) };
    if requested != CLAP_PLUGIN_FACTORY_ID {
        return ptr::null();
    }
    ptr::from_ref(&PLUGIN_FACTORY).cast()
}

unsafe extern "C" fn factory_plugin_count(_factory: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    if index == 0 {
        ptr::from_ref(&PLUGIN_DESCRIPTOR)
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn factory_create_plugin(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if host.is_null() || plugin_id.is_null() {
        return ptr::null();
    }
    // SAFETY: CLAP supplies a NUL-terminated plugin identifier.
    if unsafe { CStr::from_ptr(plugin_id) } != PLUGIN_ID {
        return ptr::null();
    }

    let large_state = match std::env::var_os(LARGE_STATE_ENV) {
        None => false,
        Some(value) if value == "1" => true,
        Some(_) => return ptr::null(),
    };
    let mut state = Box::new(PluginState::new(host, large_state));
    let plugin = Box::new(clap_plugin {
        desc: ptr::from_ref(&PLUGIN_DESCRIPTOR),
        plugin_data: ptr::from_mut(&mut *state).cast::<c_void>(),
        init: Some(plugin_init),
        destroy: Some(plugin_destroy),
        activate: Some(plugin_activate),
        deactivate: Some(plugin_deactivate),
        start_processing: Some(plugin_start_processing),
        stop_processing: Some(plugin_stop_processing),
        reset: Some(plugin_reset),
        process: Some(plugin_process),
        get_extension: Some(plugin_get_extension),
        on_main_thread: Some(plugin_on_main_thread),
    });
    let _state = Box::into_raw(state);
    Box::into_raw(plugin).cast_const()
}

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    let host = state.host;
    // SAFETY: the host pointer is retained from create_plugin and remains live through destroy.
    let Some(host) = (unsafe { host.as_ref() }) else {
        return false;
    };
    let Some(get_extension) = host.get_extension else {
        return true;
    };
    // SAFETY: the host callback is valid for the host object paired with this instance.
    let extension = unsafe { get_extension(state.host, CLAP_EXT_LATENCY.as_ptr()) };
    state.host_latency.store(
        extension.cast_mut().cast::<clap_host_latency>(),
        Ordering::Release,
    );
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    // SAFETY: create_plugin returned this exact boxed API object and CLAP calls destroy once.
    let plugin_box = unsafe { Box::from_raw(plugin.cast_mut()) };
    let state = plugin_box.plugin_data.cast::<PluginState>();
    // SAFETY: create_plugin stores this separately allocated state until this destroy callback.
    if !state.is_null() {
        unsafe { drop(Box::from_raw(state)) };
    }
    drop(plugin_box);
}

unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    _sample_rate: f64,
    _min_frames_count: u32,
    _max_frames_count: u32,
) -> bool {
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    let requested = state.requested_delay_frames.load(Ordering::Acquire);
    let previous = state.active_delay_frames.swap(requested, Ordering::AcqRel);
    // SAFETY: CLAP serializes activation against processing and reset for this instance.
    let Some(ring) = (unsafe { state.ring.get().as_mut() }) else {
        return false;
    };
    ring.reset();
    state.active.store(true, Ordering::Release);
    if previous != requested {
        // Activation is a main-thread lifecycle callback; publish latency only after committing
        // the matching active delay used by subsequent audio processing.
        unsafe { notify_host_latency_changed(state) };
    }
    true
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    // SAFETY: this callback receives a live plugin object created by this factory.
    if let Some(state) = unsafe { plugin_state(plugin) } {
        state.active.store(false, Ordering::Release);
    }
}

unsafe extern "C" fn plugin_start_processing(plugin: *const clap_plugin) -> bool {
    // SAFETY: this callback receives a live plugin object created by this factory.
    unsafe { plugin_state(plugin) }.is_some()
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    // SAFETY: CLAP serializes reset against processing for this instance.
    if let Some(state) = unsafe { plugin_state(plugin) } {
        // SAFETY: reset is serialized with process by the CLAP lifecycle contract.
        if let Some(ring) = unsafe { state.ring.get().as_mut() } {
            ring.reset();
        }
    }
}

unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> i32 {
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return CLAP_PROCESS_ERROR;
    };
    // SAFETY: CLAP supplies a live process structure for this callback.
    let Some(process) = (unsafe { process.as_ref() }) else {
        return CLAP_PROCESS_ERROR;
    };
    let Ok(frame_count) = usize::try_from(process.frames_count) else {
        return CLAP_PROCESS_ERROR;
    };
    let config_id = state.current_config_id.load(Ordering::Acquire);
    let Some(config) = audio_config(config_id) else {
        return CLAP_PROCESS_ERROR;
    };
    // SAFETY: the CLAP host supplies the declared process arrays and they remain live through
    // this callback. The helper validates every bus and channel shape before returning pointers.
    let Some(channels) = (unsafe { process_channels(process, config) }) else {
        return CLAP_PROCESS_ERROR;
    };

    // SAFETY: activation/deactivation/reset are serialized with processing for this instance.
    let Some(ring) = (unsafe { state.ring.get().as_mut() }) else {
        return CLAP_PROCESS_ERROR;
    };
    let delay_frames =
        usize::try_from(state.active_delay_frames.load(Ordering::Acquire)).unwrap_or(usize::MAX);
    if delay_frames > MAX_DELAY_FRAMES_USIZE {
        return CLAP_PROCESS_ERROR;
    }
    unsafe {
        process_audio(
            state,
            ring,
            &ProcessBlock {
                input_channels: channels.input,
                sidechain_channels: channels.sidechain,
                output_channels: channels.output,
                channel_count: channels.channel_count,
                has_sidechain: channels.has_sidechain,
                frame_count,
                events: process.in_events,
                delay_frames,
            },
        );
    }
    CLAP_PROCESS_CONTINUE
}

/// Resolves one validated CLAP process block into main, sidechain, and output channel pointers.
///
/// # Safety
/// Every non-null bus pointer must reference an array containing the declared CLAP buses, and
/// each f32 channel pointer must remain valid for the complete processing callback.
unsafe fn process_channels(process: &clap_process, config: AudioConfig) -> Option<ProcessChannels> {
    let channel_count = config.channels_usize();
    if channel_count == 0
        || process.audio_inputs_count != config.input_bus_count()
        || process.audio_outputs_count != 1
        || process.audio_inputs.is_null()
        || process.audio_outputs.is_null()
    {
        return None;
    }

    // SAFETY: caller guarantees each process array contains the matching declared buses.
    let inputs =
        unsafe { std::slice::from_raw_parts(process.audio_inputs, config.input_bus_count_usize()) };
    // SAFETY: caller guarantees this selected configuration has one output bus.
    let outputs = unsafe { std::slice::from_raw_parts(process.audio_outputs, 1) };
    // SAFETY: the host's process records own valid f32 buffers for this callback.
    let input = unsafe { channel_pointers(&inputs[0], channel_count)? };
    let output = unsafe { channel_pointers(&outputs[0], channel_count)? };
    let sidechain = if config.has_sidechain {
        // SAFETY: the selected config and checked input count include a second bus.
        unsafe { channel_pointers(&inputs[1], channel_count)? }
    } else {
        [ptr::null_mut(); 2]
    };
    Some(ProcessChannels {
        input,
        sidechain,
        output,
        channel_count,
        has_sidechain: config.has_sidechain,
    })
}

/// Reads channel pointers only after confirming the selected bus matches the fixture's layout.
///
/// # Safety
/// `bus.data32` must point to an array with at least `channel_count` entries when non-null.
unsafe fn channel_pointers(bus: &clap_audio_buffer, channel_count: usize) -> Option<[*mut f32; 2]> {
    if usize::try_from(bus.channel_count).ok()? != channel_count
        || bus.data32.is_null()
        || !bus.data64.is_null()
    {
        return None;
    }
    let mut channels = [ptr::null_mut(); 2];
    for (index, channel) in channels.iter_mut().take(channel_count).enumerate() {
        // SAFETY: caller guarantees the f32 pointer array has `channel_count` entries.
        *channel = unsafe { *bus.data32.add(index) };
        if channel.is_null() {
            return None;
        }
    }
    Some(channels)
}

unsafe fn process_audio(state: &PluginState, ring: &mut DelayRing, block: &ProcessBlock) {
    let event_count = unsafe { input_event_count(block.events) };
    let mut event_index = 0_u32;
    let mut frame = 0_usize;
    let mut gains = current_gains(state);

    while event_index < event_count {
        let Some(header) = (unsafe { input_event(block.events, event_index) }) else {
            event_index += 1;
            continue;
        };
        // SAFETY: the event header is owned by the host list and remains valid for the callback.
        let header_ref = unsafe { &*header };
        let event_frame = usize::try_from(header_ref.time).unwrap_or(usize::MAX);
        if event_frame > block.frame_count {
            break;
        }
        // SAFETY: process_audio receives the valid CLAP buffers for this block.
        unsafe { process_segment(ring, block, frame, event_frame.max(frame), gains) };
        frame = event_frame.max(frame);

        if is_parameter_value_event(header_ref) {
            // SAFETY: the event header type and size were validated above.
            unsafe { apply_parameter_event(state, header) };
            gains = current_gains(state);
        }
        event_index += 1;
    }

    // SAFETY: process_audio receives the valid CLAP buffers for this block.
    unsafe { process_segment(ring, block, frame, block.frame_count, gains) };
}

fn is_parameter_value_event(header: &clap_event_header) -> bool {
    header.space_id == CLAP_CORE_EVENT_SPACE_ID
        && header.type_ == CLAP_EVENT_PARAM_VALUE
        && usize::try_from(header.size).unwrap_or(0)
            >= std::mem::size_of::<clap_event_param_value>()
}

fn current_gains(state: &PluginState) -> [f32; 2] {
    channel_gains(
        f64::from_bits(state.gain_a_bits.load(Ordering::Acquire)),
        f64::from_bits(state.gain_b_bits.load(Ordering::Acquire)),
    )
}

fn channel_gains(gain_a: f64, gain_b: f64) -> [f32; 2] {
    // CLAP gain values are finite and bounded; f32 is the audio sample format. Each parameter
    // scales its corresponding channel so the two controls remain distinguishable.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let left_gain = gain_a as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let right_gain = gain_b as f32;
    [left_gain, right_gain]
}

unsafe fn process_segment(
    ring: &mut DelayRing,
    block: &ProcessBlock,
    start_frame: usize,
    end_frame: usize,
    gains: [f32; 2],
) {
    for frame in start_frame..end_frame {
        // SAFETY: caller received every advertised main input channel for the whole block.
        let input = unsafe {
            [
                *block.input_channels[0].add(frame),
                if block.channel_count == 2 {
                    *block.input_channels[1].add(frame)
                } else {
                    0.0
                },
            ]
        };
        let delayed = ring.process_frame(input, block.delay_frames);
        for channel in 0..block.channel_count {
            let sidechain = if block.has_sidechain {
                // SAFETY: sidechain pointers were validated for all channels and frames.
                unsafe { *block.sidechain_channels[channel].add(frame) }
            } else {
                0.0
            };
            let output = mix_sample(delayed[channel], gains[channel], sidechain);
            // SAFETY: host supplies writable output buffers for all advertised channels/frames.
            unsafe { block.output_channels[channel].add(frame).write(output) };
        }
    }
}

fn mix_sample(delayed_main: f32, gain: f32, sidechain: f32) -> f32 {
    delayed_main * gain + sidechain * 0.5
}

fn current_parameter_value(state: &PluginState, parameter_id: clap_id) -> Option<f64> {
    match parameter_id {
        PARAM_GAIN_A => Some(f64::from_bits(state.gain_a_bits.load(Ordering::Acquire))),
        PARAM_GAIN_B => Some(f64::from_bits(state.gain_b_bits.load(Ordering::Acquire))),
        PARAM_DELAY_FRAMES => Some(f64::from(
            state.requested_delay_frames.load(Ordering::Acquire),
        )),
        _ => None,
    }
}

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    extension_id: *const c_char,
) -> *const c_void {
    if extension_id.is_null() {
        return ptr::null();
    }
    // SAFETY: CLAP passes a NUL-terminated extension identifier.
    let extension_id = unsafe { CStr::from_ptr(extension_id) };
    if extension_id == clap_sys::ext::audio_ports::CLAP_EXT_AUDIO_PORTS {
        ptr::from_ref(&AUDIO_PORTS_EXTENSION).cast()
    } else if extension_id == CLAP_EXT_AUDIO_PORTS_CONFIG {
        ptr::from_ref(&AUDIO_PORTS_CONFIG_EXTENSION).cast()
    } else if extension_id == CLAP_EXT_AUDIO_PORTS_CONFIG_INFO {
        ptr::from_ref(&AUDIO_PORTS_CONFIG_INFO_EXTENSION).cast()
    } else if extension_id == CLAP_EXT_PARAMS {
        ptr::from_ref(&PARAMS_EXTENSION).cast()
    } else if extension_id == CLAP_EXT_LATENCY {
        ptr::from_ref(&LATENCY_EXTENSION).cast()
    } else if extension_id == CLAP_EXT_STATE {
        ptr::from_ref(&STATE_EXTENSION).cast()
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

#[derive(Clone, Copy)]
struct AudioConfig {
    id: clap_id,
    name: &'static [u8],
    channel_count: u32,
    has_sidechain: bool,
}

impl AudioConfig {
    fn input_bus_count(self) -> u32 {
        if self.has_sidechain { 2 } else { 1 }
    }

    fn input_bus_count_usize(self) -> usize {
        if self.has_sidechain { 2 } else { 1 }
    }

    fn channels_usize(self) -> usize {
        match self.channel_count {
            1 => 1,
            2 => 2,
            _ => 0,
        }
    }
}

const AUDIO_CONFIGS: [AudioConfig; 3] = [
    AudioConfig {
        id: CONFIG_STEREO,
        name: b"Stereo",
        channel_count: 2,
        has_sidechain: false,
    },
    AudioConfig {
        id: CONFIG_STEREO_SIDECHAIN,
        name: b"Stereo + Sidechain",
        channel_count: 2,
        has_sidechain: true,
    },
    AudioConfig {
        id: CONFIG_MONO,
        name: b"Mono",
        channel_count: 1,
        has_sidechain: false,
    },
];

fn audio_config(config_id: clap_id) -> Option<AudioConfig> {
    AUDIO_CONFIGS
        .iter()
        .copied()
        .find(|config| config.id == config_id)
}

#[derive(Clone, Copy)]
struct PortSpec {
    id: clap_id,
    name: &'static [u8],
    is_main: bool,
    channel_count: u32,
}

fn port_spec(config: AudioConfig, index: u32, is_input: bool) -> Option<PortSpec> {
    if is_input && index == 0 {
        return Some(PortSpec {
            id: 0,
            name: if config.channel_count == 1 {
                b"Mono Input"
            } else {
                b"Main Input"
            },
            is_main: true,
            channel_count: config.channel_count,
        });
    }
    if is_input && config.has_sidechain && index == 1 {
        return Some(PortSpec {
            id: 1,
            name: b"Sidechain Input",
            is_main: false,
            channel_count: config.channel_count,
        });
    }
    if !is_input && index == 0 {
        return Some(PortSpec {
            id: 0,
            name: if config.channel_count == 1 {
                b"Mono Output"
            } else {
                b"Main Output"
            },
            is_main: true,
            channel_count: config.channel_count,
        });
    }
    None
}

fn port_type(channel_count: u32) -> *const c_char {
    match channel_count {
        1 => CLAP_PORT_MONO.as_ptr(),
        2 => CLAP_PORT_STEREO.as_ptr(),
        _ => ptr::null(),
    }
}

unsafe extern "C" fn audio_ports_count(plugin: *const clap_plugin, is_input: bool) -> u32 {
    // SAFETY: CLAP passes the live plugin object paired with this callback.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return 0;
    };
    let config_id = state.current_config_id.load(Ordering::Acquire);
    let Some(config) = audio_config(config_id) else {
        return 0;
    };
    if is_input {
        config.input_bus_count()
    } else {
        1
    }
}

unsafe extern "C" fn audio_ports_get(
    plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if info.is_null() {
        return false;
    }
    // SAFETY: CLAP passes the live plugin object paired with this callback.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    let config_id = state.current_config_id.load(Ordering::Acquire);
    let Some(config) = audio_config(config_id) else {
        return false;
    };
    let Some(port) = port_spec(config, index, is_input) else {
        return false;
    };
    // SAFETY: host supplies writable storage for one clap_audio_port_info.
    unsafe {
        info.write(clap_audio_port_info {
            id: port.id,
            name: name_buffer::<CLAP_NAME_SIZE>(port.name),
            flags: if port.is_main {
                CLAP_AUDIO_PORT_IS_MAIN
            } else {
                0
            },
            channel_count: port.channel_count,
            port_type: port_type(port.channel_count),
            in_place_pair: if port.is_main {
                port.id
            } else {
                CLAP_INVALID_ID
            },
        });
    }
    true
}

unsafe extern "C" fn audio_ports_config_count(_plugin: *const clap_plugin) -> u32 {
    u32::try_from(AUDIO_CONFIGS.len()).unwrap_or(0)
}

unsafe extern "C" fn audio_ports_config_get(
    _plugin: *const clap_plugin,
    index: u32,
    output: *mut clap_audio_ports_config,
) -> bool {
    let Ok(index) = usize::try_from(index) else {
        return false;
    };
    let Some(config) = AUDIO_CONFIGS.get(index).copied() else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies writable storage for one configuration summary.
    unsafe {
        output.write(clap_audio_ports_config {
            id: config.id,
            name: name_buffer::<CLAP_NAME_SIZE>(config.name),
            input_port_count: config.input_bus_count(),
            output_port_count: 1,
            has_main_input: true,
            main_input_channel_count: config.channel_count,
            main_input_port_type: port_type(config.channel_count),
            has_main_output: true,
            main_output_channel_count: config.channel_count,
            main_output_port_type: port_type(config.channel_count),
        });
    }
    true
}

unsafe extern "C" fn audio_ports_config_select(
    plugin: *const clap_plugin,
    config_id: clap_id,
) -> bool {
    let Some(config) = audio_config(config_id) else {
        return false;
    };
    // SAFETY: CLAP passes the live plugin object paired with this callback.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    if state.active.load(Ordering::Acquire) {
        return false;
    }
    state.current_config_id.store(config.id, Ordering::Release);
    true
}

unsafe extern "C" fn audio_ports_config_current(plugin: *const clap_plugin) -> clap_id {
    // SAFETY: CLAP passes the live plugin object paired with this callback.
    unsafe { plugin_state(plugin) }.map_or(CLAP_INVALID_ID, |state| {
        state.current_config_id.load(Ordering::Acquire)
    })
}

unsafe extern "C" fn audio_ports_config_info_get(
    _plugin: *const clap_plugin,
    config_id: clap_id,
    index: u32,
    is_input: bool,
    output: *mut clap_audio_port_info,
) -> bool {
    let Some(config) = audio_config(config_id) else {
        return false;
    };
    let Some(port) = port_spec(config, index, is_input) else {
        return false;
    };
    if output.is_null() {
        return false;
    }
    // SAFETY: CLAP supplies writable storage for one audio-port description.
    unsafe {
        output.write(clap_audio_port_info {
            id: port.id,
            name: name_buffer::<CLAP_NAME_SIZE>(port.name),
            flags: if port.is_main {
                CLAP_AUDIO_PORT_IS_MAIN
            } else {
                0
            },
            channel_count: port.channel_count,
            port_type: port_type(port.channel_count),
            in_place_pair: if port.is_main {
                port.id
            } else {
                CLAP_INVALID_ID
            },
        });
    }
    true
}

unsafe extern "C" fn params_count(_plugin: *const clap_plugin) -> u32 {
    u32::try_from(PARAMETER_SPECS.len()).unwrap_or(0)
}

unsafe extern "C" fn params_get_info(
    _plugin: *const clap_plugin,
    parameter_index: u32,
    info: *mut clap_param_info,
) -> bool {
    let Ok(index) = usize::try_from(parameter_index) else {
        return false;
    };
    let Some(spec) = PARAMETER_SPECS.get(index) else {
        return false;
    };
    if info.is_null() {
        return false;
    }
    // SAFETY: host supplies writable storage for one clap_param_info.
    unsafe {
        info.write(clap_param_info {
            id: spec.id,
            flags: spec.flags,
            cookie: ptr::null_mut(),
            name: name_buffer::<CLAP_NAME_SIZE>(spec.name),
            module: [0; CLAP_PATH_SIZE],
            min_value: spec.min,
            max_value: spec.max,
            default_value: spec.default,
        });
    }
    true
}

unsafe extern "C" fn params_get_value(
    plugin: *const clap_plugin,
    parameter_id: clap_id,
    output: *mut f64,
) -> bool {
    if output.is_null() {
        return false;
    }
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    let Some(value) = current_parameter_value(state, parameter_id) else {
        return false;
    };
    // SAFETY: host supplies writable storage for one f64.
    unsafe { output.write(value) };
    true
}

unsafe extern "C" fn params_flush(
    plugin: *const clap_plugin,
    input: *const clap_input_events,
    _output: *const clap_sys::events::clap_output_events,
) {
    // SAFETY: this callback receives a live plugin object created by this factory.
    if let Some(state) = unsafe { plugin_state(plugin) } {
        // SAFETY: CLAP supplies a live event list for the duration of flush.
        unsafe { flush_parameter_events(input, state) };
    }
}

unsafe extern "C" fn latency_get(plugin: *const clap_plugin) -> u32 {
    // SAFETY: this callback receives a live plugin object created by this factory.
    unsafe { plugin_state(plugin) }
        .map_or(0, |state| state.active_delay_frames.load(Ordering::Acquire))
}

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    let bytes = state.parameter_values().encode();
    // SAFETY: CLAP supplies a live output stream for the duration of state.save.
    if !unsafe { write_stream_all(stream, &bytes) } {
        return false;
    }
    // SAFETY: the same live stream remains owned by the host until save returns.
    !state.large_state || unsafe { large_state::write_payload(stream) }
}

unsafe fn write_stream_all(stream: *const clap_ostream, bytes: &[u8]) -> bool {
    // SAFETY: CLAP supplies either null or a live output stream during state.save.
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return false;
    };
    let Some(write) = stream.write else {
        return false;
    };
    let mut offset = 0;
    while offset < bytes.len() {
        let Ok(remaining) = u64::try_from(bytes.len() - offset) else {
            return false;
        };
        // SAFETY: the CLAP stream callback accepts a readable byte span of the supplied size.
        let written =
            unsafe { write(stream, bytes[offset..].as_ptr().cast::<c_void>(), remaining) };
        let Ok(written) = usize::try_from(written) else {
            return false;
        };
        if written == 0 || written > bytes.len() - offset {
            return false;
        }
        offset += written;
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    // SAFETY: this callback receives a live plugin object created by this factory.
    let Some(state) = (unsafe { plugin_state(plugin) }) else {
        return false;
    };
    // SAFETY: CLAP supplies a live input stream for the duration of state.load.
    let Some(values) = (unsafe { decode_native_state(stream, state.large_state) }) else {
        return false;
    };
    set_parameter(state, PARAM_GAIN_A, values.gain_a);
    set_parameter(state, PARAM_GAIN_B, values.gain_b);
    set_parameter(state, PARAM_DELAY_FRAMES, f64::from(values.delay_frames));
    true
}

unsafe fn decode_native_state(
    stream: *const clap_istream,
    large_state: bool,
) -> Option<ParameterValues> {
    let mut bytes = [0_u8; STATE_SIZE];
    // SAFETY: the stream remains live throughout state.load.
    if !unsafe { read_stream_exact(stream, &mut bytes) } {
        return None;
    }
    let values = ParameterValues::decode(&bytes)?;
    // Check the complete payload before state_load applies any parameters.
    if large_state && !unsafe { large_state::validate_payload(stream) } {
        return None;
    }
    Some(values)
}

unsafe fn read_stream_exact(stream: *const clap_istream, output: &mut [u8]) -> bool {
    // SAFETY: CLAP passes either null or a live input stream for the duration of state.load.
    let Some(stream) = (unsafe { stream.as_ref() }) else {
        return false;
    };
    let Some(read) = stream.read else {
        return false;
    };
    let mut offset = 0;
    while offset < output.len() {
        let Ok(remaining) = u64::try_from(output.len() - offset) else {
            return false;
        };
        // SAFETY: output[offset..] is a writable buffer of the advertised size.
        let read_count = unsafe {
            read(
                stream,
                output[offset..].as_mut_ptr().cast::<c_void>(),
                remaining,
            )
        };
        let Ok(read_count) = usize::try_from(read_count) else {
            return false;
        };
        if read_count == 0 || read_count > output.len() - offset {
            return false;
        }
        offset += read_count;
    }
    true
}

struct StaticFeatures([*const c_char; 2]);

// SAFETY: every pointer targets a static, immutable, NUL-terminated CLAP feature string.
unsafe impl Sync for StaticFeatures {}

static FEATURES: StaticFeatures =
    StaticFeatures([CLAP_PLUGIN_FEATURE_AUDIO_EFFECT.as_ptr(), ptr::null()]);

static PLUGIN_DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: PLUGIN_ID.as_ptr(),
    name: c"MixRelay Native CLAP Qualification Fixture".as_ptr(),
    vendor: c"MixRelay".as_ptr(),
    url: c"https://mixrelay.io".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"0.1.0".as_ptr(),
    description: c"Deterministic stereo gain and latency fixture".as_ptr(),
    features: FEATURES.0.as_ptr(),
};

static PLUGIN_FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_plugin_count),
    get_plugin_descriptor: Some(factory_plugin_descriptor),
    create_plugin: Some(factory_create_plugin),
};

static AUDIO_PORTS_EXTENSION: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

static AUDIO_PORTS_CONFIG_EXTENSION: clap_plugin_audio_ports_config =
    clap_plugin_audio_ports_config {
        count: Some(audio_ports_config_count),
        get: Some(audio_ports_config_get),
        select: Some(audio_ports_config_select),
    };

static AUDIO_PORTS_CONFIG_INFO_EXTENSION: clap_plugin_audio_ports_config_info =
    clap_plugin_audio_ports_config_info {
        current_config: Some(audio_ports_config_current),
        get: Some(audio_ports_config_info_get),
    };

static PARAMS_EXTENSION: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: None,
    text_to_value: None,
    flush: Some(params_flush),
};

static LATENCY_EXTENSION: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};

static STATE_EXTENSION: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

/// The CLAP entry point exported by the fixture dynamic library.
#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_contract_has_independent_unit_gains_and_stepped_latency() {
        assert_eq!(PARAMETER_SPECS.len(), 3);
        assert_eq!(PARAMETER_SPECS[0].id, PARAM_GAIN_A);
        assert_eq!(PARAMETER_SPECS[0].default.to_bits(), 1.0_f64.to_bits());
        assert_eq!(PARAMETER_SPECS[1].id, PARAM_GAIN_B);
        assert_eq!(PARAMETER_SPECS[1].default.to_bits(), 1.0_f64.to_bits());
        assert_eq!(PARAMETER_SPECS[2].id, PARAM_DELAY_FRAMES);
        assert_eq!(
            PARAMETER_SPECS[2].max.to_bits(),
            MAX_DELAY_FRAMES_F64.to_bits()
        );
        assert_ne!(PARAMETER_SPECS[2].flags & CLAP_PARAM_IS_STEPPED, 0);
    }

    #[test]
    fn parameter_inputs_are_finite_and_bounded() {
        assert_eq!(sanitize_gain(-2.0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(sanitize_gain(0.625).to_bits(), 0.625_f64.to_bits());
        assert_eq!(sanitize_gain(2.0).to_bits(), 1.0_f64.to_bits());
        assert_eq!(sanitize_gain(f64::NAN).to_bits(), 0.0_f64.to_bits());
        assert_eq!(sanitize_delay(-2.0), 0);
        assert_eq!(sanitize_delay(4.6), 5);
        assert_eq!(sanitize_delay(400.0), MAX_DELAY_FRAMES);
        assert_eq!(sanitize_delay(f64::INFINITY), 0);
    }

    #[test]
    fn gain_parameters_scale_left_and_right_channels_independently() {
        let gains = channel_gains(0.5, 0.25);
        let output = [2.0_f32 * gains[0], 2.0_f32 * gains[1]];
        assert_eq!(
            output.map(f32::to_bits),
            [1.0_f32, 0.5_f32].map(f32::to_bits)
        );
    }

    #[test]
    fn native_audio_configurations_describe_stereo_sidechain_and_mono_layouts() {
        let stereo = audio_config(CONFIG_STEREO).expect("stereo configuration");
        let sidechain = audio_config(CONFIG_STEREO_SIDECHAIN).expect("sidechain configuration");
        let mono = audio_config(CONFIG_MONO).expect("mono configuration");

        assert_eq!(stereo.input_bus_count(), 1);
        assert_eq!(stereo.channel_count, 2);
        assert_eq!(sidechain.input_bus_count(), 2);
        assert_eq!(sidechain.channel_count, 2);
        assert_eq!(
            port_spec(sidechain, 0, true).map(|port| (port.is_main, port.channel_count)),
            Some((true, 2))
        );
        assert_eq!(
            port_spec(sidechain, 1, true).map(|port| (port.is_main, port.channel_count)),
            Some((false, 2))
        );
        assert!(port_spec(sidechain, 2, true).is_none());
        assert_eq!(mono.input_bus_count(), 1);
        assert_eq!(mono.channel_count, 1);
        // CLAP specifies a C string value, not pointer identity across constants.
        // SAFETY: supported widths return static, non-null C string literals.
        let mono_type = unsafe { CStr::from_ptr(port_type(mono.channel_count)) };
        let stereo_type = unsafe { CStr::from_ptr(port_type(stereo.channel_count)) };
        assert_eq!(mono_type, CLAP_PORT_MONO);
        assert_eq!(stereo_type, CLAP_PORT_STEREO);
        assert!(port_type(3).is_null());
    }

    #[test]
    fn sidechain_mix_keeps_source_and_gain_channels_independent() {
        let gains = channel_gains(0.5, 0.25);
        let output = [
            mix_sample(2.0, gains[0], 0.2),
            mix_sample(2.0, gains[1], 0.4),
        ];
        let expected = [2.0_f32 * 0.5 + 0.2 * 0.5, 2.0_f32 * 0.25 + 0.4 * 0.5];
        assert_eq!(output.map(f32::to_bits), expected.map(f32::to_bits));
        assert_ne!(output[0].to_bits(), output[1].to_bits());
    }

    #[test]
    fn state_payload_round_trips_and_rejects_invalid_headers_or_delay() {
        let values = ParameterValues {
            gain_a: 0.5,
            gain_b: 0.25,
            delay_frames: MAX_DELAY_FRAMES,
        };
        let restored = ParameterValues::decode(&values.encode());
        assert_eq!(
            restored.map(|value| {
                (
                    value.gain_a.to_bits(),
                    value.gain_b.to_bits(),
                    value.delay_frames,
                )
            }),
            Some((
                values.gain_a.to_bits(),
                values.gain_b.to_bits(),
                values.delay_frames
            ))
        );

        let mut invalid_magic = values.encode();
        invalid_magic[0] ^= 1;
        assert!(ParameterValues::decode(&invalid_magic).is_none());

        let mut invalid_delay = values.encode();
        invalid_delay[24..28].copy_from_slice(&(MAX_DELAY_FRAMES + 1).to_le_bytes());
        assert!(ParameterValues::decode(&invalid_delay).is_none());
    }

    #[test]
    fn delay_ring_has_exact_frame_count_and_reset_starts_silent() {
        let mut ring = DelayRing::new();
        assert_eq!(
            ring.process_frame([1.0, -1.0], 2).map(f32::to_bits),
            [0.0_f32; 2].map(f32::to_bits)
        );
        assert_eq!(
            ring.process_frame([2.0, -2.0], 2).map(f32::to_bits),
            [0.0_f32; 2].map(f32::to_bits)
        );
        assert_eq!(
            ring.process_frame([3.0, -3.0], 2).map(f32::to_bits),
            [1.0_f32, -1.0].map(f32::to_bits)
        );
        ring.reset();
        assert_eq!(
            ring.process_frame([4.0, -4.0], 2).map(f32::to_bits),
            [0.0_f32; 2].map(f32::to_bits)
        );
    }
}
