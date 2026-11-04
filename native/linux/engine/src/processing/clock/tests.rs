use super::*;
use crate::runtime::ENGINE_SAMPLE_RATE;

#[test]
fn rate_mapping_preserves_exact_duration_with_variable_quantums() {
    for rate in [44_100, 48_000, 96_000] {
        let mut timeline = GraphTimeline::default();
        let mut input = 0;
        let mut output = 0;
        for quantum in [128, 256, 1024, 127].into_iter().cycle().take(10_000) {
            let cycle = timeline
                .map(&DriverCycle {
                    position: input,
                    monotonic_ns: 0,
                    clock_id: 1,
                    rate,
                    frames: quantum,
                })
                .unwrap();
            assert_eq!(cycle.first_frame, output);
            assert_eq!(cycle.discontinuity, input == 0);
            input += u64::from(quantum);
            output += u64::from(cycle.frames);
        }
        assert_eq!(
            output,
            input * u64::from(ENGINE_SAMPLE_RATE) / u64::from(rate)
        );
    }
}

#[test]
fn clock_change_reanchors_remainder_and_invalid_callback_preserves_state() {
    let mut timeline = GraphTimeline::default();
    let native = DriverCycle {
        position: 0,
        monotonic_ns: 0,
        clock_id: 1,
        rate: 44_100,
        frames: 128,
    };
    let first = timeline.map(&native).unwrap();
    assert!(timeline.map(&DriverCycle { rate: 0, ..native }).is_err());
    let changed = timeline
        .map(&DriverCycle {
            clock_id: 2,
            ..native
        })
        .unwrap();
    assert_eq!(first.frames, changed.frames);
    assert!(changed.discontinuity);
    assert_eq!(changed.clock.epoch, first.clock.epoch + 1);
    assert_eq!(changed.first_frame, u64::from(first.frames));
}

fn native(position: u64, clock_id: u32, rate: u32) -> DriverCycle {
    DriverCycle {
        position,
        monotonic_ns: position,
        clock_id,
        rate,
        frames: 128,
    }
}

fn event(number: u64) -> NativeDiscontinuity {
    NativeDiscontinuity {
        previous: native(number * 128, 1, 48_000),
        current: native(number * 128 + 128, 2, 44_100),
        mapped_epoch: number + 1,
        render_position: number * 128,
    }
}

#[test]
fn native_timeline_events_capture_gap_clock_and_rate_changes() {
    let mut timeline = GraphTimeline::default();
    timeline.map(&native(0, 1, 48_000)).unwrap();
    assert_eq!(timeline.take_discontinuity(), None);
    timeline.map(&native(128, 1, 48_000)).unwrap();
    assert_eq!(timeline.take_discontinuity(), None);

    let gap = timeline.map(&native(512, 1, 48_000)).unwrap();
    assert_eq!(gap.clock.epoch, 2);
    assert_eq!(
        timeline.take_discontinuity(),
        Some(NativeDiscontinuity {
            previous: native(128, 1, 48_000),
            current: native(512, 1, 48_000),
            mapped_epoch: 2,
            render_position: 256,
        })
    );
    assert_eq!(timeline.take_discontinuity(), None);

    timeline.map(&native(640, 2, 48_000)).unwrap();
    let changed_clock = timeline.take_discontinuity().unwrap();
    assert_eq!(changed_clock.previous, native(512, 1, 48_000));
    assert_eq!(changed_clock.current, native(640, 2, 48_000));
    assert_eq!(changed_clock.mapped_epoch, 3);
    timeline.map(&native(768, 2, 44_100)).unwrap();
    let changed_rate = timeline.take_discontinuity().unwrap();
    assert_eq!(changed_rate.previous, native(640, 2, 48_000));
    assert_eq!(changed_rate.current, native(768, 2, 44_100));
    assert_eq!(changed_rate.mapped_epoch, 4);
}

#[test]
fn invalid_native_cycle_does_not_advance_diagnostic_previous() {
    let mut timeline = GraphTimeline::default();
    timeline.map(&native(0, 1, 48_000)).unwrap();
    assert!(timeline.map(&native(128, 1, 0)).is_err());
    timeline.map(&native(128, 1, 48_000)).unwrap();
    assert_eq!(timeline.take_discontinuity(), None);
    assert!(timeline.map(&native(u64::MAX, 1, 48_000)).is_err());
    timeline.map(&native(512, 1, 48_000)).unwrap();
    assert_eq!(
        timeline.take_discontinuity().unwrap().previous,
        native(128, 1, 48_000)
    );
}

#[test]
fn zero_frame_mapper_pending_discontinuity_is_reported_only_once() {
    let mut timeline = GraphTimeline::new(8_000).unwrap();
    let native = DriverCycle {
        position: 0,
        monotonic_ns: 0,
        clock_id: 1,
        rate: 192_000,
        frames: 1,
    };
    assert_eq!(timeline.map(&native).unwrap().frames, 0);
    assert_eq!(timeline.take_discontinuity(), None);

    let gap = DriverCycle {
        position: 1_000,
        ..native
    };
    let mapped_gap = timeline.map(&gap).unwrap();
    assert_eq!(mapped_gap.frames, 0);
    assert_eq!(mapped_gap.clock.epoch, 2);
    assert_eq!(timeline.take_discontinuity().unwrap().current, gap);

    let mut saw_render_frame = false;
    for position in 1_001..1_030 {
        let contiguous = DriverCycle { position, ..native };
        let cycle = timeline.map(&contiguous).unwrap();
        saw_render_frame |= cycle.frames > 0;
        assert_eq!(timeline.take_discontinuity(), None);
    }
    assert!(saw_render_frame);
}

#[test]
fn mailbox_reports_overwrite_and_suppresses_duplicate_read() {
    let mailbox = DiscontinuityMailbox::default();
    assert_eq!(mailbox.read_since(0), None);
    mailbox.publish(event(1));
    assert_eq!(
        mailbox.read_since(0),
        Some(DiscontinuityRead {
            event_number: 1,
            overwritten: 0,
            event: event(1),
        })
    );
    assert_eq!(mailbox.read_since(1), None);
    mailbox.publish(event(2));
    mailbox.publish(event(3));
    assert_eq!(
        mailbox.read_since(1),
        Some(DiscontinuityRead {
            event_number: 3,
            overwritten: 1,
            event: event(3),
        })
    );
}

#[test]
fn mailbox_snapshot_is_coherent_during_single_writer_updates() {
    use std::sync::Arc;

    let mailbox = Arc::new(DiscontinuityMailbox::default());
    let writer = Arc::clone(&mailbox);
    let worker = std::thread::spawn(move || {
        for number in 1..=10_000 {
            writer.publish(event(number));
            if number % 8 == 0 {
                std::thread::yield_now();
            }
        }
    });
    let mut last = 0;
    for _ in 0..10_000 {
        if let Some(read) = mailbox.read_since(last) {
            assert_eq!(read.event, event(read.event_number));
            assert_eq!(read.overwritten, read.event_number - last - 1);
            last = read.event_number;
        }
        std::thread::yield_now();
    }
    worker.join().unwrap();
    if let Some(read) = mailbox.read_since(last) {
        assert_eq!(read.event, event(read.event_number));
        last = read.event_number;
    }
    assert_eq!(last, 10_000);
}
