use super::{Demand, Ordering};

#[test]
fn deadlines_reject_duplicates_and_reset_only_as_a_pair() {
    let demand = Demand::new();
    demand.publish(256, 256);
    demand.publish(128, 32);
    demand.publish(256, 64);
    assert_eq!(demand.read(), Some((256, 256)));
    demand.reset();
    assert_eq!(demand.read(), Some((0, 0)));
    demand.publish(32, 32);
    assert_eq!(demand.read(), Some((32, 32)));
}

#[test]
fn contending_and_exhausted_publishers_do_not_wait_or_corrupt_state() {
    let demand = Demand::new();
    demand.sequence.store(1, Ordering::Relaxed);
    demand.publish(32, 32);
    assert_eq!(demand.read(), None);
    demand.sequence.store(u64::MAX - 1, Ordering::Relaxed);
    demand.publish(64, 64);
    assert_eq!(demand.read(), Some((0, 0)));
}

#[test]
fn concurrent_observers_never_combine_two_callback_windows() {
    let demand = Demand::new();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for frame in 1..=100_000_u32 {
                demand.publish(u64::from(frame), frame);
            }
        });
        for _ in 0..100_000 {
            if let Some((stamp, frames)) = demand.read() {
                assert_eq!(stamp, u64::from(frames));
            }
        }
    });
    assert_eq!(demand.read(), Some((100_000, 100_000)));
}
