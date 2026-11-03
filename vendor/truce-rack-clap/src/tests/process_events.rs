//! Plugin trait calls with prepared parameter event storage.

use super::*;
use crate::events::make_param_value_event;
use truce_rack_core::events::{Event, EventBody};

struct EventCalls {
    calls: AtomicUsize,
    valid: AtomicBool,
}

unsafe extern "C" fn start(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn process_events(plugin: *const clap_plugin, block: *const clap_process) -> i32 {
    // SAFETY: the fixture keeps native records and its atomic counters alive throughout the call.
    let (state, block) = unsafe { (&*(*plugin).plugin_data.cast::<EventCalls>(), &*block) };
    // SAFETY: the adapter's live block owns complete event-list records for this callback.
    let input = unsafe { &*block.in_events };
    let mut valid = unsafe { input.size.unwrap()(block.in_events) } == 33;
    for index in 0..33 {
        let header = unsafe { input.get.unwrap()(block.in_events, index) };
        if header.is_null() {
            valid = false;
            continue;
        }
        // SAFETY: these fixture inputs are complete parameter events built by the adapter.
        let parameter: clap_sys::events::clap_event_param_value =
            unsafe { std::ptr::read_unaligned(header.cast()) };
        valid &= parameter.param_id == if index == 0 { 7 } else { 8 };
    }
    let event = make_param_value_event(0, 9, 0.75);
    // SAFETY: the host output list and local event remain live for this synchronous callback.
    valid &=
        unsafe { (*block.out_events).try_push.unwrap()(block.out_events, &raw const event.header) };
    state.valid.fetch_and(valid, Ordering::Relaxed);
    state.calls.fetch_add(1, Ordering::Relaxed);
    CLAP_PROCESS_CONTINUE
}

fn with_stereo_buffer(visit: impl FnOnce(&mut AudioBuffer<'_, f32>)) {
    let input = [0.25_f32; PROCESS_TEST_FRAMES];
    let mut left = [0.0_f32; PROCESS_TEST_FRAMES];
    let mut right = [0.0_f32; PROCESS_TEST_FRAMES];
    let inputs = [&input[..], &input[..]];
    let mut outputs = [&mut left[..], &mut right[..]];
    let ranges = [BusRange::new(0, 2)];
    let mut buffer = AudioBuffer::new(&inputs, &mut outputs, PROCESS_TEST_FRAMES, &ranges, &ranges);
    visit(&mut buffer);
}

#[test]
fn actual_plugin_process_converts_and_emits_events_without_per_block_heap_use() {
    let calls = EventCalls {
        calls: AtomicUsize::new(0),
        valid: AtomicBool::new(true),
    };
    // SAFETY: nullable native fields accept zero; only the installed callbacks are invoked.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = (&raw const calls).cast_mut().cast();
    native.start_processing = Some(start);
    native.process = Some(process_events);
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    instance.parameters.ids = ParameterIds::collect(1, |_| {
        Ok(ParameterMetadata {
            id: 7,
            requires_process: true,
        })
    })
    .unwrap();
    let host_event = Event {
        sample_offset: 1,
        body: EventBody::ParamValue {
            param_id: 8,
            value: 0.25,
        },
    };
    let events = EventList::from_slice(&[host_event; 32]);
    let mut output = EventList::try_with_capacity(4096).unwrap();
    with_stereo_buffer(|buffer| {
        let mut context = ProcessContext {
            sample_rate: 48_000.0,
            max_block_size: PROCESS_TEST_FRAMES,
            transport: None,
            output_events: &mut output,
        };
        let measured = allocation_counter::measure(|| {
            for _ in 0..1000 {
                instance.parameters.ids.queue(0, 0.5).unwrap();
                context.output_events.clear();
                assert_eq!(
                    instance.process(buffer, &events, &mut context).unwrap(),
                    ProcessStatus::Continue
                );
                assert_eq!(context.output_events.len(), 1);
            }
        });
        assert_eq!(measured.count_total, 0);
    });
    assert_eq!(calls.calls.load(Ordering::Relaxed), 1000);
    assert!(calls.valid.load(Ordering::Relaxed));
    assert!(!instance.parameters.ids.has_pending());
    instance.processing.active_layout = None;
    instance.processing.started_processing = false;
}

#[test]
fn oversized_event_block_never_starts_native_processing_or_advances_steady_time() {
    let counters = ProcessCallCounters {
        starts: AtomicUsize::new(0),
        processes: AtomicUsize::new(0),
    };
    // SAFETY: nullable native fields accept zero; counters back the installed native callbacks.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = (&raw const counters).cast_mut().cast();
    native.start_processing = Some(counted_start_processing);
    native.process = Some(counted_process);
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    let event = Event {
        sample_offset: 0,
        body: EventBody::ParamValue {
            param_id: 8,
            value: 0.25,
        },
    };
    let mut events = EventList::try_with_capacity(4097).unwrap();
    for _ in 0..4097 {
        events.push(event);
    }
    let mut output = EventList::new();
    with_stereo_buffer(|buffer| {
        let mut context = ProcessContext {
            sample_rate: 48_000.0,
            max_block_size: PROCESS_TEST_FRAMES,
            transport: None,
            output_events: &mut output,
        };
        let measured = allocation_counter::measure(|| {
            assert_eq!(
                instance.process(buffer, &events, &mut context).unwrap(),
                ProcessStatus::Error
            );
        });
        assert_eq!(measured.count_total, 0);
    });
    assert_eq!(counters.starts.load(Ordering::Relaxed), 0);
    assert_eq!(counters.processes.load(Ordering::Relaxed), 0);
    assert_eq!(instance.processing.steady_time, 0);
    instance.processing.active_layout = None;
}

unsafe extern "C" fn reject_start(plugin: *const clap_plugin) -> bool {
    // SAFETY: the native fixture retains its counters through the rejected start call.
    let counters = unsafe { &*(*plugin).plugin_data.cast::<ProcessCallCounters>() };
    counters.starts.fetch_add(1, Ordering::Relaxed);
    false
}

fn assert_start_failure_without_allocation(
    start_callback: Option<unsafe extern "C" fn(*const clap_plugin) -> bool>,
    expected_fault: truce_rack_core::ProcessFault,
    expected_starts: usize,
) {
    let counters = ProcessCallCounters {
        starts: AtomicUsize::new(0),
        processes: AtomicUsize::new(0),
    };
    // SAFETY: nullable native fields accept zero; installed callbacks retain valid counters.
    let mut native: clap_plugin = unsafe { std::mem::zeroed() };
    native.plugin_data = (&raw const counters).cast_mut().cast();
    native.start_processing = start_callback;
    native.process = Some(counted_process);
    let mut instance = active_process_test_instance(&native, process_layout(2, 2));
    let events = EventList::new();
    let mut output = EventList::new();
    with_stereo_buffer(|buffer| {
        let mut context = ProcessContext {
            sample_rate: 48_000.0,
            max_block_size: PROCESS_TEST_FRAMES,
            transport: None,
            output_events: &mut output,
        };
        let mut outcome = None;
        let measured = allocation_counter::measure(|| {
            outcome = Some(instance.process(buffer, &events, &mut context));
        });
        assert_eq!(
            measured.count_total, 0,
            "native start failures must not allocate on the audio thread"
        );
        assert!(
            matches!(outcome, Some(Err(Error::Process { format: FORMAT, fault })) if fault == expected_fault)
        );
    });
    assert_eq!(counters.starts.load(Ordering::Relaxed), expected_starts);
    assert_eq!(counters.processes.load(Ordering::Relaxed), 0);
    assert_eq!(instance.processing.steady_time, 0);
    assert!(!instance.processing.started_processing);
    instance.processing.active_layout = None;
}

#[test]
fn missing_start_callback_reports_a_typed_fault_without_audio_thread_allocation() {
    assert_start_failure_without_allocation(
        None,
        truce_rack_core::ProcessFault::MissingCallback,
        0,
    );
}

#[test]
fn rejected_native_start_reports_a_typed_fault_without_audio_thread_allocation() {
    assert_start_failure_without_allocation(
        Some(reject_start),
        truce_rack_core::ProcessFault::NativeStatus(0),
        1,
    );
}
