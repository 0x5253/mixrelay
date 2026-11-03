use super::*;
use std::cell::Cell;

#[test]
fn publications_coalesce_and_only_a_consumed_token_reopens_them() {
    let notifications = Notifications::default();
    let signals = Cell::new(0);
    for now in [10, 20, 30] {
        notifications.publish(
            || now,
            || {
                signals.set(signals.get() + 1);
                0
            },
        );
    }
    assert_eq!(signals.get(), 1);
    assert_eq!(notifications.received(|_| Wake::TimedOut), Wake::TimedOut);
    notifications.publish(|| 40, || panic!("pending publications must coalesce"));
    assert_eq!(
        notifications.received(|published_at| Wake::Notified { published_at }),
        Wake::Notified { published_at: 10 }
    );
    notifications.publish(
        || 50,
        || {
            signals.set(signals.get() + 1);
            0
        },
    );
    assert_eq!(signals.get(), 2);
    assert_eq!(
        notifications.received(|published_at| Wake::Notified { published_at }),
        Wake::Notified { published_at: 50 }
    );
}
#[test]
fn failed_signal_is_counted_and_the_next_producer_can_retry() {
    let notifications = Notifications::default();
    notifications.publish(|| 10, || 5);
    assert_eq!(notifications.failed_signals(), 1);
    assert_eq!(notifications.received(|_| Wake::Failed(5)), Wake::Failed(5));
    notifications.publish(|| 20, || 0);
    assert_eq!(
        notifications.received(|published_at| Wake::Notified { published_at }),
        Wake::Notified { published_at: 20 }
    );
    assert_eq!(notifications.failed_signals(), 1);
}
