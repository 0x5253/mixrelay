use super::*;
use truce_rack_core::events::{Event as RackEvent, EventBody, MidiData};
use vst3::Steinberg::{
    Vst::{Event, IEventListTrait},
    kResultOk,
};

#[test]
fn block_event_exchange_reuses_com_objects_and_storage() {
    let mut exchange = EventExchange::new().unwrap();
    let events = EventList::from_slice(&[RackEvent {
        sample_offset: 2,
        body: EventBody::Midi(MidiData::NoteOn {
            channel: 1,
            note: 60,
            velocity: 127,
        }),
    }]);
    let mut output_events = EventList::try_with_capacity(MAX_EVENTS).unwrap();
    let allocations = crate::test_support::count_heap_operations(|| {
        for _ in 0..64 {
            assert_eq!(
                exchange.process(&events, 128, &mut output_events, |input, output| {
                    let input = unsafe { vst3::ComRef::from_raw(input) }.unwrap();
                    let output = unsafe { vst3::ComRef::from_raw(output) }.unwrap();
                    assert_eq!(unsafe { input.getEventCount() }, 1);
                    assert_eq!(unsafe { output.getEventCount() }, 0);
                    let mut event = std::mem::MaybeUninit::<Event>::uninit();
                    assert_eq!(unsafe { input.getEvent(0, event.as_mut_ptr()) }, kResultOk);
                    let event = unsafe { event.assume_init() };
                    assert_eq!(event.sampleOffset, 2);
                    assert_eq!(unsafe { event.__field0.noteOn.pitch }, 60);
                    kResultOk
                }),
                Ok(kResultOk)
            );
        }
    });
    assert_eq!(
        allocations,
        (0, 0),
        "per-block event translation must retain its COM objects and memory"
    );
}

fn note(offset: u32) -> RackEvent {
    RackEvent {
        sample_offset: offset,
        body: EventBody::Midi(MidiData::NoteOn {
            channel: 1,
            note: 60,
            velocity: 127,
        }),
    }
}

fn native_note(offset: u32) -> Event {
    translation::translate(&note(offset), 128).unwrap().unwrap()
}

#[test]
fn input_capacity_and_bad_coordinates_reject_before_native_processing() {
    let mut exchange = EventExchange::new().unwrap();
    let mut output = EventList::try_with_capacity(MAX_EVENTS).unwrap();
    let mut events = EventList::try_with_capacity(MAX_EVENTS + 1).unwrap();
    for _ in 0..MAX_EVENTS {
        assert!(events.try_push(note(127)));
    }
    assert_eq!(exchange.process(&events, 128, &mut output, |_, _| 7), Ok(7));
    assert!(events.try_push(note(127)));
    assert_eq!(
        exchange.process(&events, 128, &mut output, |_, _| panic!(
            "overflow called native DSP"
        )),
        Err(ProcessFault::InputEventCapacity)
    );
    for invalid in [
        note(128),
        note(u32::MAX),
        RackEvent {
            sample_offset: 0,
            body: EventBody::Midi(MidiData::NoteOff {
                channel: 16,
                note: 60,
                velocity: 127,
            }),
        },
    ] {
        assert_eq!(
            exchange.process(
                &EventList::from_slice(&[invalid]),
                128,
                &mut output,
                |_, _| panic!("malformed input called native DSP")
            ),
            Err(ProcessFault::InvalidInputEvent)
        );
    }
    assert_eq!(
        exchange.process(&EventList::new(), 128, &mut output, |input, _| {
            assert_eq!(
                unsafe { vst3::ComRef::from_raw(input).unwrap().getEventCount() },
                0
            );
            9
        }),
        Ok(9)
    );
}

#[test]
fn native_output_limit_and_ignored_add_failure_are_observed_without_heap_use() {
    let mut exchange = EventExchange::new().unwrap();
    let mut output = EventList::try_with_capacity(MAX_EVENTS).unwrap();
    let (allocations, frees) = crate::test_support::count_heap_operations(|| {
        assert_eq!(
            exchange.process(&EventList::new(), 128, &mut output, |_, out| {
                let mut event = native_note(0);
                for _ in 0..MAX_EVENTS {
                    assert_eq!(
                        unsafe {
                            vst3::ComRef::from_raw(out)
                                .unwrap()
                                .addEvent(&raw mut event)
                        },
                        kResultOk
                    );
                }
                assert_eq!(
                    unsafe { vst3::ComRef::from_raw(out).unwrap().getEventCount() },
                    i32::try_from(MAX_EVENTS).unwrap()
                );
                assert_ne!(
                    unsafe {
                        vst3::ComRef::from_raw(out)
                            .unwrap()
                            .addEvent(&raw mut event)
                    },
                    kResultOk
                );
                kResultOk
            }),
            Err(ProcessFault::OutputEventCapacity)
        );
    });
    assert_eq!((allocations, frees), (0, 0));
    assert!(output.is_empty());
    assert_eq!(
        exchange.process(&EventList::new(), 128, &mut output, |_, out| {
            assert_eq!(
                unsafe { vst3::ComRef::from_raw(out).unwrap().getEventCount() },
                0
            );
            let mut event = native_note(7);
            assert_eq!(
                unsafe {
                    vst3::ComRef::from_raw(out)
                        .unwrap()
                        .addEvent(&raw mut event)
                },
                kResultOk
            );
            kResultOk
        }),
        Ok(kResultOk)
    );
    assert_eq!(output.len(), 1);
    assert_eq!(output.as_slice()[0].sample_offset, 7);
    assert!(matches!(
        output.as_slice()[0].body,
        EventBody::Midi(MidiData::NoteOn {
            channel: 1,
            note: 60,
            velocity: 127
        })
    ));
}

#[test]
fn prepared_host_sink_refuses_overflow_and_next_block_clears_accepted_prefix() {
    let mut exchange = EventExchange::new().unwrap();
    let mut output = EventList::new();
    let count = crate::test_support::count_heap_operations(|| {
        assert_eq!(
            exchange.process(&EventList::new(), 128, &mut output, |_, out| {
                for offset in 0..33 {
                    let mut event = native_note(offset);
                    assert_eq!(
                        unsafe {
                            vst3::ComRef::from_raw(out)
                                .unwrap()
                                .addEvent(&raw mut event)
                        },
                        kResultOk
                    );
                }
                kResultOk
            }),
            Err(ProcessFault::OutputEventCapacity)
        );
    });
    assert_eq!(count, (0, 0));
    assert_eq!(output.len(), 32);
    assert_eq!(output.as_slice()[0].sample_offset, 0);
    assert_eq!(
        exchange.process(&EventList::new(), 128, &mut output, |_, _| kResultOk),
        Ok(kResultOk)
    );
    assert!(output.is_empty());
}

#[test]
fn retained_output_interface_owns_storage_after_exchange_drop() {
    let exchange = EventExchange::new().unwrap();
    assert!(exchange.output.reset(128));
    let retained = exchange.output_ptr.clone();
    drop(exchange);
    std::thread::spawn(move || {
        let mut event = native_note(3);
        assert_eq!(unsafe { retained.addEvent(&raw mut event) }, kResultOk);
        let mut read = std::mem::MaybeUninit::<Event>::uninit();
        assert_eq!(
            unsafe { retained.getEvent(0, read.as_mut_ptr()) },
            kResultOk
        );
        assert_eq!(unsafe { read.assume_init() }.sampleOffset, 3);
        assert_ne!(
            unsafe { retained.getEvent(-1, read.as_mut_ptr()) },
            kResultOk
        );
    })
    .join()
    .unwrap();
}

#[test]
fn malformed_native_output_is_rejected_even_when_processor_ignores_status() {
    let mut exchange = EventExchange::new().unwrap();
    let mut output = EventList::new();
    let mut invalid = [native_note(0); 5];
    invalid[0].sampleOffset = 128;
    invalid[1].sampleOffset = -1;
    invalid[2].busIndex = -1;
    invalid[3].ppqPosition = f64::NAN;
    invalid[4].__field0.noteOn.velocity = f32::NAN;
    for mut event in invalid {
        assert_eq!(
            exchange.process(&EventList::new(), 128, &mut output, |_, out| {
                let pointer = unsafe { vst3::ComRef::from_raw(out) }.unwrap();
                assert_ne!(unsafe { pointer.addEvent(&raw mut event) }, kResultOk);
                kResultOk
            }),
            Err(ProcessFault::InvalidOutputEvent)
        );
        assert!(output.is_empty());
    }
    assert_eq!(
        exchange.process(&EventList::new(), 128, &mut output, |_, _| kResultOk),
        Ok(kResultOk)
    );
}
