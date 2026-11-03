use super::*;

fn buffers() -> AtomBuffers {
    let map = UridMap::new();
    let mut buffers = AtomBuffers::new(100, 101, &map, [0].into_iter());
    buffers.prepare(1_024).unwrap();
    buffers
}

fn write_sequence(buffers: &mut AtomBuffers, count: u32) {
    let start = std::mem::size_of::<LV2AtomSequence>();
    let slot = 24;
    let bytes = start + count as usize * slot;
    assert!(bytes <= buffers.outputs[0].buffer.len());
    // SAFETY: the fixture writes complete aligned headers and padded 3-byte payloads
    // inside the same prepared output buffer that a native LV2 run would receive.
    unsafe {
        let sequence = &mut *buffers.outputs[0]
            .buffer
            .as_mut_ptr()
            .cast::<LV2AtomSequence>();
        sequence.atom = LV2Atom {
            size: (bytes - 8) as u32,
            mytype: buffers.sequence_urid,
        };
        sequence.body = LV2AtomSequenceBody { unit: 0, pad: 0 };
        for index in 0..count {
            let pointer = buffers.outputs[0]
                .buffer
                .as_mut_ptr()
                .byte_add(start + index as usize * slot);
            pointer.cast::<LV2AtomEvent>().write(LV2AtomEvent {
                time_in_frames: i64::from(index),
                body: LV2Atom {
                    size: 3,
                    mytype: buffers.midi_event_urid,
                },
            });
            pointer
                .byte_add(std::mem::size_of::<LV2AtomEvent>())
                .cast::<u8>()
                .copy_from_nonoverlapping([0x90, 60, 100].as_ptr(), 3);
        }
    }
}

fn sequence(buffers: &mut AtomBuffers) -> &mut LV2AtomSequence {
    // SAFETY: every test owner prepares a complete aligned header before this borrow.
    unsafe {
        &mut *buffers.outputs[0]
            .buffer
            .as_mut_ptr()
            .cast::<LV2AtomSequence>()
    }
}

#[test]
fn output_advertises_full_chunk_capacity_and_invalidates_previous_events() {
    let mut buffers = buffers();
    write_sequence(&mut buffers, 1);
    buffers.prepare_output();
    let chunk = buffers.chunk_urid;
    let capacity = buffers.outputs[0].buffer.len() - std::mem::size_of::<LV2Atom>();
    let header = sequence(&mut buffers);
    assert_eq!(header.atom.mytype, chunk);
    assert_eq!(header.atom.size as usize, capacity);
    let mut events = EventList::new();
    buffers.drain_output(&mut events, 128).unwrap();
    assert!(events.is_empty());
}

#[test]
fn aligned_native_events_cross_inline_capacity_without_realtime_allocation() {
    let mut buffers = buffers();
    assert_eq!(buffers.input.as_ptr().align_offset(8), 0);
    assert_eq!(buffers.outputs[0].buffer.as_ptr().align_offset(8), 0);
    write_sequence(&mut buffers, 33);
    let mut events = EventList::try_with_capacity(33).unwrap();
    let allocations = crate::test_allocations::measure(|| {
        buffers.drain_output(&mut events, 128).unwrap();
    });
    assert_eq!(allocations, 0);
    assert_eq!(events.len(), 33);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sample_offset as usize, index);
        assert!(matches!(
            event.body,
            EventBody::Midi(MidiData::NoteOn {
                note: 60,
                velocity: 100,
                ..
            })
        ));
    }
}

#[test]
fn full_host_event_list_preserves_accepted_prefix_without_growing() {
    let mut buffers = buffers();
    write_sequence(&mut buffers, 33);
    let mut events = EventList::new();
    let mut result = Ok(());
    let allocations = crate::test_allocations::measure(|| {
        result = buffers.drain_output(&mut events, 128);
    });
    assert_eq!(result, Err(AtomFault::OutputCapacity));
    assert_eq!(allocations, 0);
    assert_eq!(events.len(), 32);
    assert_eq!(events.as_slice().last().unwrap().sample_offset, 31);
}

#[test]
fn invalid_sequence_extent_type_and_time_units_are_rejected_before_payload_reads() {
    let mut buffers = buffers();
    let mut events = EventList::new();
    for fault in 0..4 {
        write_sequence(&mut buffers, 1);
        let header = sequence(&mut buffers);
        match fault {
            0 => header.atom.size = u32::MAX,
            1 => header.atom.size = 7,
            2 => header.atom.mytype = u32::MAX,
            3 => header.body.unit = u32::MAX,
            _ => unreachable!(),
        }
        assert_eq!(
            buffers.drain_output(&mut events, 128),
            Err(AtomFault::InvalidSequence)
        );
        assert!(events.is_empty());
    }
}

#[test]
fn truncated_payloads_and_invalid_frame_offsets_are_rejected() {
    let mut buffers = buffers();
    let mut events = EventList::new();
    for fault in 0..4 {
        write_sequence(&mut buffers, 1);
        // SAFETY: write_sequence creates this complete aligned first event header.
        let event = unsafe {
            &mut *buffers.outputs[0]
                .buffer
                .as_mut_ptr()
                .byte_add(16)
                .cast::<LV2AtomEvent>()
        };
        match fault {
            0 => event.body.size = u32::MAX,
            1 => event.time_in_frames = -1,
            2 => event.time_in_frames = 128,
            3 => sequence(&mut buffers).atom.size -= 1,
            _ => unreachable!(),
        }
        assert_eq!(
            buffers.drain_output(&mut events, 128),
            Err(AtomFault::InvalidEvent)
        );
        assert!(events.is_empty());
    }
}

#[test]
fn independent_atom_owners_do_not_share_storage_or_previous_output() {
    let mut first = buffers();
    let mut second = buffers();
    write_sequence(&mut first, 2);
    write_sequence(&mut second, 1);
    first.prepare_output();
    let mut events = EventList::new();
    second.drain_output(&mut events, 128).unwrap();
    assert_eq!(events.len(), 1);
    assert_ne!(first.input.as_ptr(), second.input.as_ptr());
    assert_ne!(
        first.outputs[0].buffer.as_ptr(),
        second.outputs[0].buffer.as_ptr()
    );
    events.clear();
    first.drain_output(&mut events, 128).unwrap();
    assert!(events.is_empty());
    second.prepare(2_048).unwrap();
    assert_eq!(first.outputs[0].buffer.len(), 1_024);
}

#[test]
fn distinct_native_output_ports_have_independent_capacity_and_merge_in_frame_order() {
    let map = UridMap::new();
    let mut buffers = AtomBuffers::new(100, 101, &map, [4, 9].into_iter());
    buffers.prepare(1_024).unwrap();
    assert_ne!(buffers.output_ptr(4), buffers.output_ptr(9));
    assert!(buffers.output_ptr(99).is_null());
    write_sequence(&mut buffers, 2);
    // Move port 9 into the fixture's first slot only while writing its native payload.
    buffers.outputs.swap(0, 1);
    write_sequence(&mut buffers, 1);
    buffers.outputs.swap(0, 1);
    // SAFETY: both complete first event headers were written by write_sequence.
    unsafe {
        let first = buffers.outputs[0]
            .buffer
            .as_mut_ptr()
            .byte_add(16)
            .cast::<LV2AtomEvent>();
        (*first).time_in_frames = 4;
        let last = buffers.outputs[0]
            .buffer
            .as_mut_ptr()
            .byte_add(40)
            .cast::<LV2AtomEvent>();
        (*last).time_in_frames = 8;
        let second = buffers.outputs[1]
            .buffer
            .as_mut_ptr()
            .byte_add(16)
            .cast::<LV2AtomEvent>();
        (*second).time_in_frames = 2;
    }
    let mut events = EventList::new();
    let allocations =
        crate::test_allocations::measure(|| buffers.drain_output(&mut events, 128).unwrap());
    assert_eq!(allocations, 0);
    assert_eq!(
        events
            .iter()
            .map(|event| event.sample_offset)
            .collect::<Vec<_>>(),
        [2, 4, 8]
    );
}

#[test]
fn input_capacity_exhaustion_is_explicit_and_process_fault_conversion_does_not_allocate() {
    let mut buffers = buffers();
    let mut input = EventList::try_with_capacity(43).unwrap();
    for frame in 0..43 {
        assert!(input.try_push(Event {
            sample_offset: frame,
            body: EventBody::Midi(MidiData::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100
            }),
        }));
    }
    let mut fault = None;
    let allocations = crate::test_allocations::measure(|| {
        fault = buffers
            .build_sequence(&input, None)
            .err()
            .map(AtomFault::into_error);
    });
    assert!(matches!(
        fault,
        Some(Error::Process {
            format: "lv2",
            fault: ProcessFault::InputEventCapacity
        })
    ));
    assert_eq!(allocations, 0);
}
