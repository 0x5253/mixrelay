use super::*;

#[test]
fn histograms_summarize_windows() {
    let mut histogram = LatencyHistogram::default();
    assert_eq!(histogram.take(), None);
    for nanoseconds in [500, 3_000, 3_500, 90_000] {
        histogram.record(nanoseconds);
    }
    // An hour-long delay lands in the last bucket.
    histogram.record(3_600_000_000_000);
    let summary = histogram.take().unwrap();
    assert_eq!(summary.count, 5);
    assert_eq!(summary.p50_us, 4);
    assert_eq!(summary.p99_us, 1 << (LATENCY_BUCKETS - 1));
    assert_eq!(summary.maximum_us, 3_600_000_000);
    assert_eq!(histogram.take(), None);
}

use std::{cell::RefCell, collections::VecDeque, rc::Rc};
#[derive(Clone, Debug)]
struct Scheduler {
    events: Rc<RefCell<Vec<&'static str>>>,
    promotes: Rc<RefCell<VecDeque<bool>>>,
    demotes: Rc<RefCell<VecDeque<bool>>>,
}
impl Scheduler {
    fn new(
        promotes: impl IntoIterator<Item = bool>,
        demotes: impl IntoIterator<Item = bool>,
    ) -> Self {
        Self {
            events: Rc::default(),
            promotes: Rc::new(RefCell::new(promotes.into_iter().collect())),
            demotes: Rc::new(RefCell::new(demotes.into_iter().collect())),
        }
    }
}
impl Scheduling for Scheduler {
    fn promote(&self, period: Duration, _: Duration, _: Duration) -> io::Result<RealtimePolicy> {
        self.events.borrow_mut().push("promote");
        if !self.promotes.borrow_mut().pop_front().unwrap() {
            return Err(io::Error::other("promote"));
        }
        Ok(RealtimePolicy {
            period_ns: u64::try_from(period.as_nanos()).unwrap(),
            computation_ns: 1,
            constraint_ns: 1,
        })
    }
    fn demote(&self) -> io::Result<()> {
        self.events.borrow_mut().push("demote");
        if self.demotes.borrow_mut().pop_front().unwrap() {
            Ok(())
        } else {
            Err(io::Error::other("demote"))
        }
    }
}
#[test]
fn control_work_runs_after_refused_demotion_without_promoting_again() {
    let scheduler = Scheduler::new([true], [false]);
    let thread = ScheduledThread::with_scheduler(
        scheduler.clone(),
        Duration::from_millis(1),
        Duration::from_nanos(1),
        Duration::from_nanos(1),
    )
    .unwrap();
    let (value, result) = thread.outside(|| {
        scheduler.events.borrow_mut().push("work");
        7
    });
    assert_eq!(value, 7);
    assert!(matches!(result, Err(Transition::Demote(_))));
    assert_eq!(*scheduler.events.borrow(), ["promote", "demote", "work"]);
}
#[test]
fn failed_promotion_restores_standard_priority_and_preserves_last_observation() {
    let scheduler = Scheduler::new([true, true, false], [true, true, true]);
    let thread = ScheduledThread::with_scheduler(
        scheduler.clone(),
        Duration::from_millis(1),
        Duration::from_nanos(1),
        Duration::from_nanos(1),
    )
    .unwrap();
    thread.retune(
        Duration::from_millis(2),
        Duration::from_nanos(1),
        Duration::from_nanos(1),
    );
    thread.outside(|| ()).1.unwrap();
    assert_eq!(thread.applied().period_ns, 2_000_000);
    assert!(matches!(
        thread.outside(|| ()).1,
        Err(Transition::Promote(_))
    ));
    assert_eq!(thread.applied().period_ns, 2_000_000);
    assert_eq!(
        *scheduler.events.borrow(),
        [
            "promote", "demote", "promote", "demote", "promote", "demote"
        ]
    );
    let scheduler = Scheduler::new([false], [true]);
    assert!(
        ScheduledThread::with_scheduler(
            scheduler.clone(),
            Duration::ZERO,
            Duration::ZERO,
            Duration::ZERO
        )
        .is_err()
    );
    assert_eq!(*scheduler.events.borrow(), ["promote", "demote"]);
}
#[test]
fn supplied_timebase_rounds_up_and_rejects_invalid_budget_before_native_requests() {
    let timebase = Timebase::from_ratio(
        std::num::NonZeroU32::new(125).unwrap(),
        std::num::NonZeroU32::new(3).unwrap(),
    );
    assert_eq!(timebase.ticks(Duration::from_nanos(42)), 2);
    assert_eq!(timebase.nanoseconds(2), 83);
    for budget in [
        (Duration::ZERO, Duration::ZERO, Duration::ZERO),
        (
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(1),
        ),
        (
            Duration::from_secs(u64::MAX),
            Duration::from_nanos(1),
            Duration::from_nanos(1),
        ),
    ] {
        assert_eq!(
            policy_request(timebase, budget.0, budget.1, budget.2)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    let request = policy_request(
        timebase,
        Duration::from_millis(1),
        Duration::from_micros(250),
        Duration::from_micros(500),
    )
    .unwrap();
    assert_eq!(
        applied_policy(timebase, (0, 0), false, request)
            .unwrap()
            .period_ns,
        1_000_000
    );
    for (codes, defaulted) in [((5, 0), false), ((0, 5), false), ((0, 0), true)] {
        assert!(applied_policy(timebase, codes, defaulted, request).is_err());
    }
}
