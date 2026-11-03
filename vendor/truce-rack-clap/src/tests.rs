//! Shared native instance fixtures and format metadata regressions.

use crate::audio_ports::AudioPortCatalog;
use crate::editor::{EditorState, handle_to_clap_window, platform_api};
use crate::events::{ConvertedInputEvents, EventStorage};
use crate::host::HostContext;
use crate::library::LoadedLibrary;
use crate::lifecycle::{
    ActivationConfig, ActivationTarget, InstanceDestruction, ProcessingState, destroy_instance,
    query_latency_frames,
};
use crate::parameter_ids::{ParameterIds, ParameterMetadata};
use crate::parameters::{
    Parameters, c_buf_to_string, commit_processed_parameter_changes,
    flush_pending_parameter_changes, queue_cached_parameter_change, read_parameter_ids,
};
use crate::process::{NativeProcessTarget, ProcessChannelPointers, run_native_process_block};
use crate::scanner::{bundle_binary_path, parse_version};
use crate::{ClapPlugin, FORMAT};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    CLAP_EVENT_NOTE_ON, clap_event_header, clap_input_events, clap_output_events,
};
use clap_sys::ext::gui::{
    CLAP_EXT_GUI, CLAP_WINDOW_API_COCOA, CLAP_WINDOW_API_WIN32, CLAP_WINDOW_API_X11,
    clap_plugin_gui, clap_window,
};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use clap_sys::ext::params::{CLAP_PARAM_REQUIRES_PROCESS, clap_param_info, clap_plugin_params};
use clap_sys::ext::timer_support::{CLAP_EXT_TIMER_SUPPORT, clap_plugin_timer_support};
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_CONTINUE, CLAP_PROCESS_ERROR, clap_process};
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::ptr;
use truce_rack_core::buffer::AudioBuffer;
use truce_rack_core::bus::BusLayout;
use truce_rack_core::error::Error;
use truce_rack_core::events::EventList;
use truce_rack_core::info::{PluginCategory, PluginInfo};
use truce_rack_core::plugin::{
    MainThreadUpdate, Plugin, PluginCore, ProcessContext, ProcessStatus,
};
mod editor;
mod lifecycle;
mod parameters;
mod process;
mod process_events;

use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::ext::timer_support::clap_host_timer_support;
use clap_sys::host::clap_host;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use truce_rack_core::buffer::BusRange;

const PROCESS_TEST_FRAMES: usize = 64;
const PROCESS_BASELINE_BLOCKS: usize = 10_000;

#[test]
fn c_buffer_conversion_preserves_bytes_and_stops_at_nul() {
    let buffer = [
        c_char::from_ne_bytes([0xC3]),
        c_char::from_ne_bytes([0xA9]),
        c_char::from_ne_bytes([0]),
        c_char::from_ne_bytes([0xFF]),
    ];

    assert_eq!(c_buf_to_string(&buffer), "é");
}

struct ProcessPointerState {
    expected_inputs: Vec<Vec<*mut f32>>,
    expected_outputs: Vec<Vec<*mut f32>>,
    process_status: i32,
    calls: AtomicUsize,
    valid: AtomicBool,
    last_status: AtomicI32,
}

impl ProcessPointerState {
    fn validate(&self, block: &clap_process) -> bool {
        let expected_steady_time = i64::try_from(self.calls.load(Ordering::Relaxed))
            .unwrap_or(i64::MAX)
            .saturating_mul(i64::try_from(PROCESS_TEST_FRAMES).unwrap_or(i64::MAX));
        if block.steady_time != expected_steady_time
            || block.frames_count != u32::try_from(PROCESS_TEST_FRAMES).unwrap_or(u32::MAX)
            || !block.transport.is_null()
            || usize::try_from(block.audio_inputs_count).unwrap_or(usize::MAX)
                != self.expected_inputs.len()
            || usize::try_from(block.audio_outputs_count).unwrap_or(usize::MAX)
                != self.expected_outputs.len()
            || block.in_events.is_null()
            || block.out_events.is_null()
        {
            return false;
        }

        // SAFETY: this boundary fixture keeps both event records alive around each native
        // call; the input callback is configured to expose no events in the baseline.
        let input_event_count = unsafe {
            let events = &*block.in_events;
            events.size.map_or(u32::MAX, |size| size(block.in_events))
        };
        if input_event_count != 0 {
            return false;
        }

        // SAFETY: this synchronous fixture keeps the records and pointed-to slices live
        // through the native callback.
        unsafe {
            audio_bus_channel_pointers_match(block.audio_inputs, &self.expected_inputs)
                && audio_bus_channel_pointers_match(
                    block.audio_outputs.cast_const(),
                    &self.expected_outputs,
                )
        }
    }
}

unsafe fn audio_bus_channel_pointers_match(
    audio: *const clap_audio_buffer,
    expected_buses: &[Vec<*mut f32>],
) -> bool {
    if expected_buses.is_empty() {
        return audio.is_null();
    }
    if audio.is_null() {
        return false;
    }
    for (bus_index, expected_channels) in expected_buses.iter().enumerate() {
        // SAFETY: the synchronous fixture supplies one live record per expected bus.
        let bus = unsafe { &*audio.add(bus_index) };
        if usize::try_from(bus.channel_count).unwrap_or(usize::MAX) != expected_channels.len()
            || bus.data32.is_null()
            || !bus.data64.is_null()
            || bus.latency != 0
            || bus.constant_mask != 0
        {
            return false;
        }
        // SAFETY: count validation established one live slot per fixture channel.
        for (channel_index, expected) in expected_channels.iter().enumerate() {
            if unsafe { *bus.data32.add(channel_index) } != *expected {
                return false;
            }
        }
    }
    true
}

struct ProcessPointerFixture {
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    input_bus_channels: Vec<usize>,
    output_bus_channels: Vec<usize>,
    process_status: i32,
}

struct ProcessPointerMeasurement {
    allocations: allocation_counter::AllocationInfo,
    calls: usize,
    valid: bool,
    scratch_cleared: bool,
    last_status: i32,
}

impl ProcessPointerFixture {
    fn measure_blocks(
        &mut self,
        layout: &BusLayout,
        block_count: usize,
    ) -> ProcessPointerMeasurement {
        let expected_inputs = channel_pointer_groups(&mut self.inputs, &self.input_bus_channels);
        let expected_outputs = channel_pointer_groups(&mut self.outputs, &self.output_bus_channels);
        let mut state = ProcessPointerState {
            expected_inputs,
            expected_outputs,
            process_status: self.process_status,
            calls: AtomicUsize::new(0),
            valid: AtomicBool::new(true),
            last_status: AtomicI32::new(CLAP_PROCESS_ERROR),
        };
        // SAFETY: zero is valid for this inert CLAP callback table; plugin_data and the
        // process callback are set before this local plugin is passed to the native helper.
        let mut plugin: clap_plugin = unsafe { std::mem::zeroed() };
        plugin.plugin_data = (&raw mut state).cast();
        plugin.process = Some(process_pointer_fixture_process);
        let host_context = HostContext::new();

        let input_channels: Vec<&[f32]> = self.inputs.iter().map(Vec::as_slice).collect();
        let mut output_channels: Vec<&mut [f32]> =
            self.outputs.iter_mut().map(Vec::as_mut_slice).collect();
        let input_ranges = bus_ranges(&self.input_bus_channels);
        let output_ranges = bus_ranges(&self.output_bus_channels);
        let mut pointers = ProcessChannelPointers::for_layout(layout).unwrap();
        let mut buffer = AudioBuffer::new(
            &input_channels,
            &mut output_channels,
            PROCESS_TEST_FRAMES,
            &input_ranges,
            &output_ranges,
        );
        let (input_events, output_events) = empty_process_event_records();
        let state = &state;
        let host_context = &host_context;
        let plugin = &raw const plugin;

        let warmup = run_native_process_block(
            NativeProcessTarget {
                plugin,
                host_context,
                steady_time: 0,
                pointers: &mut pointers,
            },
            &mut buffer,
            None,
            &input_events,
            &output_events,
        );
        state
            .valid
            .store(warmup == CLAP_PROCESS_CONTINUE, Ordering::Relaxed);
        state.last_status.store(warmup, Ordering::Relaxed);

        let allocations = allocation_counter::measure(|| {
            for block_index in 0..block_count {
                let steady_time = i64::try_from(block_index + 1)
                    .unwrap_or(i64::MAX)
                    .saturating_mul(i64::try_from(PROCESS_TEST_FRAMES).unwrap_or(i64::MAX));
                let status = run_native_process_block(
                    NativeProcessTarget {
                        plugin,
                        host_context,
                        steady_time,
                        pointers: &mut pointers,
                    },
                    &mut buffer,
                    None,
                    &input_events,
                    &output_events,
                );
                state
                    .valid
                    .fetch_and(status == CLAP_PROCESS_CONTINUE, Ordering::Relaxed);
                state.last_status.store(status, Ordering::Relaxed);
            }
        });
        ProcessPointerMeasurement {
            allocations,
            calls: state.calls.load(Ordering::Relaxed),
            valid: state.valid.load(Ordering::Relaxed),
            scratch_cleared: pointers_are_clear(&pointers),
            last_status: state.last_status.load(Ordering::Relaxed),
        }
    }

    fn outputs_match_inputs(&self) -> bool {
        self.inputs == self.outputs
    }
}

fn pointers_are_clear(pointers: &ProcessChannelPointers) -> bool {
    pointers
        .input_channels
        .iter()
        .chain(&pointers.output_channels)
        .flatten()
        .all(|pointer| pointer.is_null())
        && pointers
            .input_buffers
            .iter()
            .chain(&pointers.output_buffers)
            .all(|buffer| buffer.data32.is_null())
}

fn channel_pointer_groups(
    channels: &mut [Vec<f32>],
    bus_channel_counts: &[usize],
) -> Vec<Vec<*mut f32>> {
    let mut channel_index = 0;
    bus_channel_counts
        .iter()
        .map(|&bus_channel_count| {
            let start = channel_index;
            channel_index += bus_channel_count;
            channels[start..channel_index]
                .iter_mut()
                .map(Vec::as_mut_ptr)
                .collect()
        })
        .collect()
}

fn bus_ranges(bus_channel_counts: &[usize]) -> Vec<BusRange> {
    let mut start = 0;
    bus_channel_counts
        .iter()
        .map(|&channel_count| {
            let range = BusRange::new(start, channel_count);
            start += channel_count;
            range
        })
        .collect()
}

unsafe extern "C" fn empty_process_input_size(_events: *const clap_input_events) -> u32 {
    0
}

unsafe extern "C" fn empty_process_input_get(
    _events: *const clap_input_events,
    _index: u32,
) -> *const clap_event_header {
    ptr::null()
}

fn empty_process_event_records() -> (clap_input_events, clap_output_events) {
    (
        clap_input_events {
            ctx: ptr::null_mut(),
            size: Some(empty_process_input_size),
            get: Some(empty_process_input_get),
        },
        clap_output_events {
            ctx: ptr::null_mut(),
            try_push: None,
        },
    )
}

unsafe extern "C" fn process_pointer_fixture_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> i32 {
    // SAFETY: plugin_data is set to the live state and the process record is supplied by
    // the synchronous production boundary call measured by this test.
    let state = unsafe { &*(*plugin).plugin_data.cast::<ProcessPointerState>() };
    if process.is_null() {
        state.valid.store(false, Ordering::Relaxed);
        return CLAP_PROCESS_ERROR;
    }
    // SAFETY: process is checked non-null and remains live until this callback returns.
    let block = unsafe { &*process };
    if !state.validate(block) {
        state.valid.store(false, Ordering::Relaxed);
        return CLAP_PROCESS_ERROR;
    }
    if !block.audio_outputs.is_null() {
        // SAFETY: validate proved each pointer matches the live test buffers; the helper
        // retains those buffers for this complete callback.
        unsafe {
            for output_bus_index in 0..usize::try_from(block.audio_outputs_count).unwrap_or(0) {
                let output_bus = &mut *block.audio_outputs.add(output_bus_index);
                let input_bus =
                    if output_bus_index < usize::try_from(block.audio_inputs_count).unwrap_or(0) {
                        Some(&*block.audio_inputs.add(output_bus_index))
                    } else {
                        None
                    };
                for channel_index in 0..usize::try_from(output_bus.channel_count).unwrap_or(0) {
                    let output = *output_bus.data32.add(channel_index);
                    let input = input_bus
                        .filter(|bus| {
                            channel_index < usize::try_from(bus.channel_count).unwrap_or(0)
                        })
                        .map_or(ptr::null_mut(), |bus| *bus.data32.add(channel_index));
                    for frame in 0..PROCESS_TEST_FRAMES {
                        if input.is_null() {
                            *output.add(frame) = 0.0;
                        } else {
                            *output.add(frame) = *input.add(frame);
                        }
                    }
                }
            }
        }
    }
    state.calls.fetch_add(1, Ordering::Relaxed);
    state.process_status
}

fn process_pointer_fixture(input_channels: usize, output_channels: usize) -> ProcessPointerFixture {
    ProcessPointerFixture {
        inputs: (0..input_channels)
            .map(|_| vec![0.25_f32; PROCESS_TEST_FRAMES])
            .collect(),
        outputs: (0..output_channels)
            .map(|_| vec![0.0_f32; PROCESS_TEST_FRAMES])
            .collect(),
        input_bus_channels: if input_channels == 0 {
            Vec::new()
        } else {
            vec![input_channels]
        },
        output_bus_channels: if output_channels == 0 {
            Vec::new()
        } else {
            vec![output_channels]
        },
        process_status: CLAP_PROCESS_CONTINUE,
    }
}

fn process_layout(input_channels: usize, output_channels: usize) -> BusLayout {
    use truce_rack_core::bus::{Bus, ChannelConfig};

    let channel_config = |channels| match channels {
        1 => ChannelConfig::Mono,
        2 => ChannelConfig::Stereo,
        channels => ChannelConfig::Discrete(u32::try_from(channels).unwrap()),
    };
    let mut layout = BusLayout::new();
    if input_channels > 0 {
        layout
            .inputs
            .push(Bus::main("Input", channel_config(input_channels)));
    }
    if output_channels > 0 {
        layout
            .outputs
            .push(Bus::main("Output", channel_config(output_channels)));
    }
    layout
}

fn stereo_sidechain_layout() -> BusLayout {
    use truce_rack_core::bus::{Bus, ChannelConfig};

    let mut layout = BusLayout::new();
    layout.inputs.push(Bus::main("Main", ChannelConfig::Stereo));
    layout
        .inputs
        .push(Bus::sidechain("Sidechain", ChannelConfig::Stereo));
    layout
        .outputs
        .push(Bus::main("Output", ChannelConfig::Stereo));
    layout
}

struct ProcessCallCounters {
    starts: AtomicUsize,
    processes: AtomicUsize,
}

unsafe extern "C" fn counted_start_processing(plugin: *const clap_plugin) -> bool {
    // SAFETY: plugin_data points to the live counter fixture used by this test.
    let counters = unsafe { &*(*plugin).plugin_data.cast::<ProcessCallCounters>() };
    counters.starts.fetch_add(1, Ordering::Relaxed);
    true
}

unsafe extern "C" fn counted_process(
    plugin: *const clap_plugin,
    _process: *const clap_process,
) -> i32 {
    // SAFETY: plugin_data points to the live counter fixture used by this test.
    let counters = unsafe { &*(*plugin).plugin_data.cast::<ProcessCallCounters>() };
    counters.processes.fetch_add(1, Ordering::Relaxed);
    CLAP_PROCESS_CONTINUE
}

fn current_process_library() -> LoadedLibrary {
    #[cfg(unix)]
    let library = libloading::os::unix::Library::this();
    #[cfg(windows)]
    let library = libloading::os::windows::Library::this().unwrap();
    LoadedLibrary {
        library: library.into(),
    }
}

fn active_process_test_instance(plugin: &clap_plugin, layout: BusLayout) -> ClapPlugin {
    let process_pointers = ProcessChannelPointers::for_layout(&layout).unwrap();
    ClapPlugin {
        info: PluginInfo {
            name: "process test".into(),
            vendor: "test".into(),
            version: 1,
            category: PluginCategory::Effect,
            path: PathBuf::new(),
            unique_id: "test.process".into(),
            format: FORMAT,
            has_editor: false,
            accepts_midi: false,
        },
        audio_port_catalog: AudioPortCatalog::test_catalog(vec![layout.clone()]),
        plugin: std::ptr::from_ref(plugin),
        host_context: HostContext::new(),
        library: current_process_library(),
        processing: ProcessingState {
            active_layout: Some(layout),
            process_pointers: Some(process_pointers),
            input_events: Some(ConvertedInputEvents::for_process(0).unwrap()),
            latency_frames: Some(0),
            ..ProcessingState::default()
        },
        parameters: Parameters {
            ids: ParameterIds::empty(),
            changed_pending: false,
            extension: ptr::null(),
        },
        editor: EditorState {
            extension: ptr::null(),
            open: false,
        },
        state_ext: ptr::null(),
        timer_ext: ptr::null(),
        latency_ext: ptr::null(),
    }
}

#[test]
fn parse_version_components() {
    assert_eq!(parse_version("1.2.3"), (1 << 16) | (2 << 8) | 3);
    assert_eq!(parse_version("0.5"), 5 << 8);
    assert_eq!(parse_version(""), 0);
}

#[test]
fn bundle_binary_macos() {
    let p = bundle_binary_path(Path::new("/tmp/MyPlugin.clap"));
    #[cfg(target_os = "macos")]
    assert!(!p.exists() || p.starts_with("/tmp/MyPlugin.clap"));
    #[cfg(not(target_os = "macos"))]
    assert_eq!(p, Path::new("/tmp/MyPlugin.clap"));
}
