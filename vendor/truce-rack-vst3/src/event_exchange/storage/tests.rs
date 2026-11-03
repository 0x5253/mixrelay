use super::*;

fn event() -> Event {
    super::super::translation::translate(
        &truce_rack_core::events::Event {
            sample_offset: 1,
            body: truce_rack_core::events::EventBody::Midi(
                truce_rack_core::events::MidiData::NoteOn {
                    channel: 0,
                    note: 64,
                    velocity: 100,
                },
            ),
        },
        128,
    )
    .unwrap()
    .unwrap()
}

#[test]
fn competing_or_reentrant_calls_fail_without_waiting_or_overwriting_slots() {
    let storage = EventStorage::new(true).unwrap();
    assert!(storage.reset(128));
    let operations = crate::test_support::count_heap_operations(|| {
        assert_eq!(storage.access(|| storage.push(event())), Some(false));
        assert_eq!(storage.faults(), 1);
        assert_eq!(storage.count.load(Ordering::Acquire), 0);
        assert!(storage.push(event()));
        assert_eq!(storage.count.load(Ordering::Acquire), 1);
    });
    assert_eq!(operations, (0, 0));
}

#[test]
fn native_null_and_unaligned_arguments_are_rejected_before_dereferencing() {
    let storage = EventStorage::new(true).unwrap();
    assert!(storage.reset(128));
    let unaligned = std::ptr::without_provenance_mut::<Event>(1);
    // SAFETY: null and unaligned addresses exercise validation only; the COM
    // boundary must return before attempting to read or write either address.
    unsafe {
        assert_eq!(storage.addEvent(std::ptr::null_mut()), kInvalidArgument);
        assert_eq!(storage.addEvent(unaligned), kInvalidArgument);
        assert_eq!(storage.getEvent(0, std::ptr::null_mut()), kInvalidArgument);
        assert_eq!(storage.getEvent(0, unaligned), kInvalidArgument);
    }
    assert_eq!(storage.count.load(Ordering::Acquire), 0);
}
