use super::*;
use crate::session::ClockAuthority;

#[test]
fn producer_jitter_does_not_move_the_shared_cursor_and_restart_reacquires() {
    let mut alignment = CaptureAlignment::default();
    let mut cycle = ProcessCycle {
        clock: ClockDomain {
            authority: ClockAuthority::Device,
            id: 7,
            epoch: 1,
        },
        sample_rate: 48_000,
        frames: 64,
        first_frame: 0,
        monotonic_ns: None,
        discontinuity: false,
    };
    let base = 1_000_000_000_u64;
    for index in 0..1000_u32 {
        let frontier = Some(base + u64::from(index / 8) * 8 * 64 * 1_000_000_000 / 48_000);
        let actual = alignment
            .begin(
                cycle,
                frontier.map(|time| CaptureFrontier::new(1, time)),
                1e9 / 48_000.0,
            )
            .unwrap();
        let expected = base + u64::from(index) * 64 * 1_000_000_000 / 48_000;
        assert!(actual.abs_diff(expected) <= 1);
        cycle.first_frame += 64;
    }
    cycle.clock.epoch += 1;
    assert_eq!(
        alignment.begin(
            cycle,
            Some(CaptureFrontier::new(1, base * 3)),
            1e9 / 48_000.0
        ),
        Some(base * 3)
    );
}

#[test]
fn a_requested_reacquisition_adopts_the_next_frontier_once() {
    let mut alignment = CaptureAlignment::default();
    let mut cycle = ProcessCycle {
        clock: ClockDomain {
            authority: ClockAuthority::Device,
            id: 3,
            epoch: 1,
        },
        sample_rate: 48_000,
        frames: 128,
        first_frame: 0,
        monotonic_ns: None,
        discontinuity: false,
    };
    let period = 1e9 / 48_000.0;
    let ahead = 2_000_000_000;
    assert_eq!(
        alignment.begin(cycle, Some(CaptureFrontier::new(1, ahead)), period),
        Some(ahead)
    );
    assert!(!alignment.reacquired());
    cycle.first_frame += 128;
    // The same source's older frontier never rewinds a continuous cursor.
    let credited = 1_000_000_000;
    let continued = alignment.begin(cycle, Some(CaptureFrontier::new(1, credited)), period);
    assert!(continued.unwrap() > ahead);
    alignment.reacquire();
    cycle.first_frame += 128;
    // Without a frontier the request stays pending instead of inventing a coordinate.
    assert_eq!(alignment.begin(cycle, None, period), None);
    assert!(!alignment.reacquired());
    cycle.first_frame += 128;
    assert_eq!(
        alignment.begin(cycle, Some(CaptureFrontier::new(1, credited)), period),
        Some(credited)
    );
    assert!(alignment.reacquired());
    cycle.first_frame += 128;
    let next = alignment.begin(cycle, Some(CaptureFrontier::new(1, credited)), period);
    assert_eq!(next, Some(credited + 2_666_667));
    assert!(!alignment.reacquired());
}
