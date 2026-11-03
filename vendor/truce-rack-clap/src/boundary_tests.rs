//! Malformed plugin event admission and real-time conversion regressions.

use crate::events::{
    ConvertedInputEvents, OutputEventsSink, make_param_value_event, output_events_try_push,
};
use crate::parameter_ids::{ParameterIds, ParameterMetadata};
use crate::state::{WriteBuffer, ostream_write};
use clap_sys::events::{
    CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
    clap_event_header,
};
use clap_sys::stream::clap_ostream;
use std::ptr;
use truce_rack_core::events::{Event, EventBody, EventList};

#[test]
fn truncated_native_event_is_rejected_without_reinterpreting_its_payload() {
    let mut event = make_param_value_event(0, 7, 0.5);
    event.header.size = u32::try_from(std::mem::size_of::<clap_event_header>()).unwrap();
    let mut events = EventList::new();
    let mut sink = OutputEventsSink::new(Some(&mut events));
    let output = sink.as_clap();
    // SAFETY: even the malformed fixture owns a complete, aligned native parameter record.
    let accepted = unsafe { output_events_try_push(&raw const output, &raw const event.header) };
    assert!(!accepted, "a truncated CLAP event must fail admission");
    assert!(events.is_empty());
}

#[test]
fn pending_parameters_and_host_events_are_converted_without_audio_thread_allocation() {
    let mut parameters = ParameterIds::collect(1, |_| {
        Ok(ParameterMetadata {
            id: 7,
            requires_process: true,
        })
    })
    .unwrap();
    parameters.queue(0, 0.5).unwrap();
    let events = EventList::from_slice(&[Event {
        sample_offset: 1,
        body: EventBody::ParamValue {
            param_id: 8,
            value: 0.25,
        },
    }]);
    let mut converted = ConvertedInputEvents::for_process(parameters.len()).unwrap();
    let measured = allocation_counter::measure(|| {
        for _ in 0..1000 {
            assert!(converted.fill_for_process(&events, &parameters));
            assert_eq!(converted.events.len(), 2);
        }
    });
    assert_eq!(
        measured.count_total, 0,
        "CLAP event conversion must use prepared storage"
    );
}

#[test]
fn output_events_reject_unknown_spaces_types_and_short_payloads() {
    for event_type in [
        CLAP_EVENT_PARAM_VALUE,
        CLAP_EVENT_NOTE_ON,
        CLAP_EVENT_NOTE_OFF,
        CLAP_EVENT_MIDI,
        u16::MAX,
    ] {
        let mut event = make_param_value_event(0, 7, 0.5);
        event.header.type_ = event_type;
        event.header.size = 0;
        let mut events = EventList::new();
        let mut sink = OutputEventsSink::new(Some(&mut events));
        let output = sink.as_clap();
        // SAFETY: the fixture retains complete native event storage through the callback.
        assert!(!unsafe { output_events_try_push(&raw const output, &raw const event.header) });
        assert!(events.is_empty());
    }
    let mut event = make_param_value_event(0, 7, 0.5);
    event.header.space_id = 42;
    let mut sink = OutputEventsSink::new(None);
    let output = sink.as_clap();
    // SAFETY: the callback receives live event/list records; unsupported space must be rejected.
    assert!(!unsafe { output_events_try_push(&raw const output, &raw const event.header) });
    assert!(!unsafe { output_events_try_push(ptr::null(), &raw const event.header) });
}

#[test]
fn native_output_burst_retains_the_reserved_prefix_and_rejects_overflow_without_allocating() {
    let mut events = EventList::try_with_capacity(4096).unwrap();
    let mut sink = OutputEventsSink::new(Some(&mut events));
    let output = sink.as_clap();
    let mut accepted = 0;
    let measured = allocation_counter::measure(|| {
        for index in 0..4100_u32 {
            let event = make_param_value_event(index, index, f64::from(index));
            // SAFETY: every synchronous callback sees a complete aligned record.
            if unsafe { output_events_try_push(&raw const output, &raw const event.header) } {
                accepted += 1;
            }
        }
    });
    assert_eq!(accepted, 4096);
    assert_eq!(measured.count_total, 0);
    assert_eq!(events.len(), 4096);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sample_offset, u32::try_from(index).unwrap());
        assert!(
            matches!(event.body, EventBody::ParamValue { param_id, .. } if param_id == u32::try_from(index).unwrap())
        );
    }
}

#[test]
fn oversized_host_input_rejection_preserves_pending_parameters_and_reuses_prepared_storage() {
    let mut parameters = ParameterIds::collect(1, |_| {
        Ok(ParameterMetadata {
            id: 7,
            requires_process: true,
        })
    })
    .unwrap();
    parameters.queue(0, 0.5).unwrap();
    let mut events = EventList::try_with_capacity(4097).unwrap();
    for index in 0..4097 {
        events.push(Event {
            sample_offset: 0,
            body: EventBody::ParamValue {
                param_id: index,
                value: 0.25,
            },
        });
    }
    let mut converted = ConvertedInputEvents::for_process(parameters.len()).unwrap();
    let measured = allocation_counter::measure(|| {
        assert!(!converted.fill_for_process(&events, &parameters));
        assert!(converted.events.is_empty());
        assert!(parameters.has_pending());
        events.clear();
        assert!(converted.fill_for_process(&events, &parameters));
        assert_eq!(converted.events.len(), 1);
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn impossible_state_write_size_fails_before_reading_the_supplied_pointer() {
    let mut buffer = WriteBuffer::default();
    let stream = clap_ostream {
        ctx: (&raw mut buffer).cast(),
        write: Some(ostream_write),
    };
    // SAFETY: the callback must reject this size before dereferencing the non-null sentinel.
    let result = unsafe {
        ostream_write(
            &raw const stream,
            std::ptr::NonNull::<u8>::dangling().as_ptr().cast(),
            u64::MAX,
        )
    };
    assert_eq!(result, -1);
    assert!(buffer.bytes.is_empty());
}
