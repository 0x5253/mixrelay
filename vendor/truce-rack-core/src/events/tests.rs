//! Event bursts remain bounded when a native producer exceeds prepared storage.
use super::*;

#[test]
fn native_event_bursts_preserve_the_accepted_prefix_without_rt_growth() {
    for capacity in [32, 128, 4096] {
        let mut list = EventList::try_with_capacity(capacity).unwrap();
        let measured = allocation_counter::measure(|| {
            for _ in 0..3 {
                for index in 0..capacity {
                    assert!(list.try_push(event(u32::try_from(index).unwrap())));
                }
                assert!(!list.try_push(event(u32::MAX)));
                for (index, event) in list.iter().enumerate() {
                    assert_eq!(event.sample_offset as usize, index);
                }
                list.clear();
            }
        });
        assert_eq!((measured.count_total, measured.count_current), (0, 0));
    }
    assert!(EventList::try_with_capacity(usize::MAX).is_err());
}

fn event(sample_offset: u32) -> Event {
    Event {
        sample_offset,
        body: EventBody::Midi(MidiData::NoteOn {
            channel: 0,
            note: 60,
            velocity: 100,
        }),
    }
}
