//! Process boundary regressions.

use super::*;

#[test]
fn production_process_boundary_reuses_prepared_scratch_without_allocating() {
    let layout = process_layout(2, 2);
    let mut fixture = process_pointer_fixture(2, 2);
    let measured = fixture.measure_blocks(&layout, PROCESS_BASELINE_BLOCKS);
    let allocations = measured.allocations;

    assert_eq!(allocations.count_total, 0);
    assert_eq!(allocations.bytes_total, 0);
    assert_eq!(allocations.count_current, 0);
    assert_eq!(allocations.bytes_current, 0);
    assert_eq!(measured.calls, PROCESS_BASELINE_BLOCKS + 1);
    assert_eq!(measured.last_status, CLAP_PROCESS_CONTINUE);
    assert!(measured.valid);
    assert!(measured.scratch_cleared);
    assert!(fixture.outputs_match_inputs());
}

#[test]
fn prepared_pointer_scratch_handles_mono_stereo_and_wide_main_buses() {
    for channel_count in [1, 2, 32, 64] {
        let layout = process_layout(channel_count, channel_count);
        let mut fixture = process_pointer_fixture(channel_count, channel_count);
        let measured = fixture.measure_blocks(&layout, 1);

        assert_eq!(
            measured.allocations.count_total, 0,
            "{channel_count} channels"
        );
        assert_eq!(
            measured.allocations.bytes_total, 0,
            "{channel_count} channels"
        );
        assert_eq!(measured.calls, 2, "{channel_count} channels");
        assert_eq!(measured.last_status, CLAP_PROCESS_CONTINUE);
        assert!(measured.valid, "{channel_count} channels");
        assert!(measured.scratch_cleared, "{channel_count} channels");
        assert!(fixture.outputs_match_inputs(), "{channel_count} channels");
    }
}

#[test]
fn prepared_pointer_scratch_keeps_main_and_sidechain_as_separate_buses() {
    let layout = stereo_sidechain_layout();
    let mut fixture = process_pointer_fixture(4, 2);
    fixture.input_bus_channels = vec![2, 2];
    let measured = fixture.measure_blocks(&layout, 1);

    assert_eq!(measured.allocations.count_total, 0);
    assert_eq!(measured.allocations.bytes_total, 0);
    assert_eq!(measured.calls, 2);
    assert_eq!(measured.last_status, CLAP_PROCESS_CONTINUE);
    assert!(measured.valid);
    assert!(measured.scratch_cleared);
    assert_eq!(&fixture.outputs, &fixture.inputs[..2]);
}

#[test]
fn mismatched_main_bus_channels_fail_without_native_process_or_allocation() {
    let layout = process_layout(2, 2);
    for (input_channels, output_channels) in [(1, 2), (2, 1)] {
        let mut fixture = process_pointer_fixture(input_channels, output_channels);
        let measured = fixture.measure_blocks(&layout, 1);

        assert_eq!(measured.allocations.count_total, 0);
        assert_eq!(measured.calls, 0);
        assert!(!measured.valid);
        assert!(measured.scratch_cleared);
        assert_eq!(measured.last_status, CLAP_PROCESS_ERROR);
    }
}

#[test]
fn actual_process_rejects_layout_mismatch_before_starting_native_processing() {
    let counters = ProcessCallCounters {
        starts: AtomicUsize::new(0),
        processes: AtomicUsize::new(0),
    };
    // SAFETY: nullable CLAP fields are valid as zero; this test installs its live callbacks.
    let mut native_plugin: clap_plugin = unsafe { std::mem::zeroed() };
    native_plugin.plugin_data = (&raw const counters).cast_mut().cast();
    native_plugin.start_processing = Some(counted_start_processing);
    native_plugin.process = Some(counted_process);
    let layout = process_layout(2, 2);
    let mut instance = active_process_test_instance(&native_plugin, layout);

    let input = [0.25_f32; PROCESS_TEST_FRAMES];
    let mut output_left = [0.0_f32; PROCESS_TEST_FRAMES];
    let mut output_right = [0.0_f32; PROCESS_TEST_FRAMES];
    let inputs = [&input[..]];
    let mut outputs = [&mut output_left[..], &mut output_right[..]];
    let input_ranges = [BusRange::new(0, 1)];
    let output_ranges = [BusRange::new(0, 2)];
    let mut buffer = AudioBuffer::new(
        &inputs,
        &mut outputs,
        PROCESS_TEST_FRAMES,
        &input_ranges,
        &output_ranges,
    );
    let events = EventList::new();
    let mut output_events = EventList::new();
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: PROCESS_TEST_FRAMES,
        transport: None,
        output_events: &mut output_events,
    };

    let status =
        <ClapPlugin as Plugin<f32>>::process(&mut instance, &mut buffer, &events, &mut context)
            .unwrap();

    assert_eq!(status, ProcessStatus::Error);
    assert_eq!(counters.starts.load(Ordering::Relaxed), 0);
    assert_eq!(counters.processes.load(Ordering::Relaxed), 0);
    instance.processing.active_layout = None;
}

#[test]
fn native_process_error_still_clears_every_pointer_slot() {
    let layout = process_layout(2, 2);
    let mut fixture = process_pointer_fixture(2, 2);
    fixture.process_status = CLAP_PROCESS_ERROR;
    let measured = fixture.measure_blocks(&layout, 1);

    assert_eq!(measured.calls, 2);
    assert_eq!(measured.last_status, CLAP_PROCESS_ERROR);
    assert!(!measured.valid);
    assert!(measured.scratch_cleared);
}

#[test]
fn generator_sink_and_asymmetric_main_buses_forward_only_live_channel_pointers() {
    for (input_channels, output_channels) in [(0, 2), (2, 0), (1, 2), (2, 1)] {
        let layout = process_layout(input_channels, output_channels);
        let mut fixture = process_pointer_fixture(input_channels, output_channels);
        let measured = fixture.measure_blocks(&layout, 1);

        assert_eq!(
            measured.calls, 2,
            "{input_channels} in, {output_channels} out"
        );
        assert_eq!(measured.last_status, CLAP_PROCESS_CONTINUE);
        assert!(measured.valid, "{input_channels} in, {output_channels} out");
        assert!(measured.scratch_cleared);
        for (input, output) in fixture.inputs.iter().zip(&fixture.outputs) {
            assert_eq!(input, output);
        }
        for output in fixture.outputs.iter().skip(fixture.inputs.len()) {
            assert!(output.iter().all(|sample| *sample == 0.0));
        }
    }
}

#[test]
fn pending_parameter_values_precede_ordered_rack_events_without_deduplication() {
    use truce_rack_core::events::{Event, EventBody, MidiData};

    let mut parameter_ids = ParameterIds::collect(1, |_| {
        Ok(ParameterMetadata {
            id: 41,
            requires_process: false,
        })
    })
    .unwrap();
    parameter_ids.queue(0, 0.75).unwrap();

    let mut rack_events = EventList::new();
    rack_events.push(Event {
        sample_offset: 0,
        body: EventBody::ParamValue {
            param_id: 41,
            value: 0.25,
        },
    });
    rack_events.push(Event {
        sample_offset: 8,
        body: EventBody::ParamValue {
            param_id: 42,
            value: 0.5,
        },
    });
    rack_events.push(Event {
        sample_offset: 24,
        body: EventBody::Midi(MidiData::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        }),
    });

    let mut converted = ConvertedInputEvents::for_process(parameter_ids.len()).unwrap();
    assert!(converted.fill_for_process(&rack_events, &parameter_ids));

    assert_eq!(converted.events.len(), 4);
    assert!(matches!(
        &converted.events[0],
        EventStorage::Param(event)
            if event.header.time == 0 && event.param_id == 41
                && event.value.to_bits() == 0.75_f64.to_bits()
    ));
    assert!(matches!(
        &converted.events[1],
        EventStorage::Param(event)
            if event.header.time == 0 && event.param_id == 41
                && event.value.to_bits() == 0.25_f64.to_bits()
    ));
    assert!(matches!(
        &converted.events[2],
        EventStorage::Param(event)
            if event.header.time == 8 && event.param_id == 42
                && event.value.to_bits() == 0.5_f64.to_bits()
    ));
    assert!(matches!(
        &converted.events[3],
        EventStorage::Note(event)
            if event.header.time == 24
                && event.header.type_ == CLAP_EVENT_NOTE_ON
                && event.key == 60
    ));
}
