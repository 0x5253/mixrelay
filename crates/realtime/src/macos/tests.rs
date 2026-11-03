use super::*;
use crate::Transition;
use std::{
    sync::{Arc, atomic::Ordering},
    thread,
    time::Instant,
};

const PERIOD: Duration = Duration::from_micros(2_667);

#[test]
fn timebase_conversions_round_up_and_invert() {
    let timebase = Timebase::current();
    let ticks = timebase.ticks(Duration::from_millis(1));
    let back = timebase.nanoseconds(ticks);
    assert!((1_000_000..1_000_000 + 100).contains(&back), "{back}");
    assert_eq!(timebase.ticks(Duration::ZERO), 0);
    let start = host_time();
    wait_until(start + timebase.ticks(Duration::from_millis(2)));
    assert!(timebase.nanoseconds(host_time() - start) >= 2_000_000);
}

#[test]
fn realtime_budgets_are_validated_before_the_kernel_sees_them() {
    for (period, computation, constraint) in [
        (
            Duration::ZERO,
            Duration::from_micros(1),
            Duration::from_micros(1),
        ),
        (PERIOD, Duration::ZERO, PERIOD),
        (PERIOD, PERIOD, PERIOD / 2),
        (PERIOD, PERIOD / 4, PERIOD * 2),
        (
            Duration::from_mins(10),
            Duration::from_secs(1),
            Duration::from_secs(2),
        ),
    ] {
        let error = promote_current_thread(period, computation, constraint).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn a_promoted_thread_reports_its_applied_policy_and_runs_intervals() {
    thread::spawn(|| {
        let policy =
            promote_current_thread(PERIOD, PERIOD * 2 / 5, PERIOD * 4 / 5).expect("promote");
        assert_eq!(policy.period_ns.div_ceil(1_000), 2_667);
        // The kernel may raise the computation budget, never past the constraint.
        assert!(
            policy.computation_ns >= 1_066_000 && policy.computation_ns <= policy.constraint_ns
        );
        let interval = WorkInterval::create(c"mixrelay.test").expect("interval");
        let joined = interval.join().expect("join");
        let timebase = Timebase::current();
        let start = host_time();
        let deadline = start + timebase.ticks(PERIOD);
        joined
            .start(start, deadline)
            .expect("start")
            .finish()
            .expect("finish");
        // A cycle abandoned by an early return still finishes, so the next one starts.
        drop(joined.start(start, deadline).expect("start"));
        let cycle = joined.start(start, deadline).expect("restart after drop");
        // Only one interval runs at a time.
        assert!(joined.start(start, deadline).is_err());
        cycle.finish().expect("finish");
    })
    .join()
    .unwrap();
}

#[test]
fn notifications_coalesce_and_carry_the_oldest_publication_time() {
    let notifier = Arc::new(Notifier::new().unwrap());
    // Publications before the wait leave one token stamped by the first of them.
    let before = host_time();
    for _ in 0..3 {
        notifier.notify();
    }
    let Wake::Notified { published_at } = notifier.wait(Duration::from_millis(50)) else {
        panic!("expected a notification");
    };
    assert!(published_at >= before && published_at <= host_time());
    assert_eq!(notifier.wait(Duration::from_millis(5)), Wake::TimedOut);
    let producer = Arc::clone(&notifier);
    let started = Instant::now();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        producer.notify();
    });
    assert!(matches!(
        notifier.wait(Duration::from_secs(5)),
        Wake::Notified { .. }
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
    handle.join().unwrap();
}

#[test]
fn kernel_and_os_failures_are_reported_not_ignored() {
    let timebase = Timebase::current();
    let applied = TimeConstraintPolicy {
        period: 1,
        computation: 1,
        constraint: 1,
        preemptible: 1,
    };
    for (codes, defaulted) in [
        ((5, KERN_SUCCESS), false),
        ((KERN_SUCCESS, 5), false),
        ((KERN_SUCCESS, KERN_SUCCESS), true),
    ] {
        assert!(applied_policy(timebase, codes, defaulted, applied).is_err());
    }
    assert!(WorkInterval::adopt(std::ptr::null_mut()).is_err());
    assert_eq!(os(22).unwrap_err().raw_os_error(), Some(22));
    assert_eq!(wake_result(5, 0), Wake::Failed(5));
    assert_eq!(wake_result(KERN_ABORTED, 0), Wake::TimedOut);
    assert!(kern(5).unwrap_err().to_string().contains('5'));
    // A failed signal reopens notification for the next producer and is counted.
    let notifier = Notifier::new().unwrap();
    notifier.notifications.publish(|| 0, || 5);
    assert_eq!(notifier.failed_signals(), 1);
    notifier.notify();
    assert!(matches!(
        notifier.wait(Duration::from_millis(50)),
        Wake::Notified { .. }
    ));
}

#[test]
fn a_realtime_thread_steps_out_for_control_work_and_returns() {
    thread::spawn(|| {
        let realtime =
            RealtimeThread::promote(PERIOD, PERIOD * 2 / 5, PERIOD * 4 / 5).expect("promote");
        assert_eq!(realtime.applied().period_ns.div_ceil(1_000), 2_667);
        let (value, restored) = realtime.outside(|| 7);
        assert_eq!(value, 7);
        restored.expect("re-enter realtime");
        // A new budget takes effect on the next re-entry.
        realtime.retune(PERIOD * 2, PERIOD * 4 / 5, PERIOD * 8 / 5);
        realtime
            .outside(|| 0)
            .1
            .expect("re-enter with the new budget");
        assert_eq!(realtime.applied().period_ns.div_ceil(1_000), 5_334);
        // An invalid budget is reported when re-entry fails.
        realtime.retune(Duration::ZERO, PERIOD, PERIOD);
        assert!(matches!(
            realtime.outside(|| 0).1,
            Err(Transition::Promote(_))
        ));
        // A refused demotion is reported as such; the thread was still realtime.
        assert!(matches!(
            realtime.restore(Err(io::Error::other("refused"))),
            Err(Transition::Demote(_))
        ));
        assert!(RealtimeThread::promote(Duration::ZERO, PERIOD, PERIOD).is_err());
        demote_current_thread().expect("demote");
    })
    .join()
    .unwrap();
}

#[test]
fn shared_futex_waits_for_a_changed_word_and_reports_outcomes() {
    assert_eq!(futex_wake(-1, 22), FutexWake::Failed(22));
    // Before macOS 14.4 the symbols are absent and callers poll.
    assert!(SharedFutex::from_symbols(std::ptr::null_mut(), std::ptr::null_mut()).is_none());
    // Exercised where the API exists (macOS 14.4 and later).
    if let Some(futex) = shared_futex() {
        let word = Arc::new(AtomicU64::new(5));
        // A stale expectation returns at once.
        assert_eq!(
            futex.wait(&word, 4, Duration::from_secs(1)),
            FutexWake::Changed
        );
        assert_eq!(
            futex.wait(&word, 5, Duration::from_millis(5)),
            FutexWake::TimedOut
        );
        let producer = Arc::clone(&word);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            producer.store(6, Ordering::Release);
            shared_futex().unwrap().wake(&producer);
        });
        assert_eq!(
            futex.wait(&word, 5, Duration::from_secs(5)),
            FutexWake::Changed
        );
        handle.join().unwrap();
    }
}
