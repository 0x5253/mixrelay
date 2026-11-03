//! Scheduling budgets and control-work transitions driven by a supplied scheduler.
use std::{cell::Cell, io, marker::PhantomData, time::Duration};

/// Applies scheduling changes to the calling thread.
pub trait Scheduling {
    /// # Errors
    /// Returns rejected or unobservable scheduling changes.
    fn promote(
        &self,
        period: Duration,
        computation: Duration,
        constraint: Duration,
    ) -> io::Result<RealtimePolicy>;
    /// # Errors
    /// Returns a failure to restore standard priority.
    fn demote(&self) -> io::Result<()>;
}

#[cfg(any(target_os = "macos", test))]
#[repr(C)]
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimeConstraintPolicy {
    pub(crate) period: u32,
    pub(crate) computation: u32,
    pub(crate) constraint: u32,
    pub(crate) preemptible: i32,
}

/// Converts between nanoseconds and host ticks with the machine's timebase.
#[derive(Debug, Clone, Copy)]
pub struct Timebase {
    pub(crate) numer: u64,
    pub(crate) denom: u64,
}

impl Timebase {
    /// Uses a supplied positive host-clock conversion ratio.
    #[must_use]
    pub fn from_ratio(numer: std::num::NonZeroU32, denom: std::num::NonZeroU32) -> Self {
        Self {
            numer: u64::from(numer.get()),
            denom: u64::from(denom.get()),
        }
    }

    /// Rounds up, so a budget is never shortened by conversion.
    #[must_use]
    pub fn ticks(self, duration: Duration) -> u64 {
        let nanoseconds = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        let ticks = u128::from(nanoseconds) * u128::from(self.denom);
        u64::try_from(ticks.div_ceil(u128::from(self.numer))).unwrap_or(u64::MAX)
    }

    #[must_use]
    pub fn nanoseconds(self, ticks: u64) -> u64 {
        let nanoseconds = u128::from(ticks) * u128::from(self.numer) / u128::from(self.denom);
        u64::try_from(nanoseconds).unwrap_or(u64::MAX)
    }
}

/// Scheduling the kernel accepted for a realtime thread, read back after the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimePolicy {
    pub period_ns: u64,
    pub computation_ns: u64,
    pub constraint_ns: u64,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn policy_request(
    timebase: Timebase,
    period: Duration,
    computation: Duration,
    constraint: Duration,
) -> io::Result<TimeConstraintPolicy> {
    if period.is_zero() || computation.is_zero() || computation > constraint || constraint > period
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "realtime budget must satisfy 0 < computation <= constraint <= period",
        ));
    }
    let period_ticks = u32::try_from(timebase.ticks(period))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "realtime period is too long"))?;
    // Validated above to fit inside the period, so these conversions cannot overflow.
    let within_period = |duration| u32::try_from(timebase.ticks(duration)).unwrap_or(period_ticks);
    Ok(TimeConstraintPolicy {
        period: period_ticks,
        computation: within_period(computation),
        constraint: within_period(constraint),
        preemptible: 1,
    })
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn applied_policy(
    timebase: Timebase,
    (set, get): (i32, i32),
    defaulted: bool,
    applied: TimeConstraintPolicy,
) -> io::Result<RealtimePolicy> {
    for result in [set, get] {
        if result != 0 {
            return Err(io::Error::other(format!("mach kernel error {result}")));
        }
    }
    if defaulted {
        return Err(io::Error::other("realtime policy was not applied"));
    }
    Ok(RealtimePolicy {
        period_ns: timebase.nanoseconds(u64::from(applied.period)),
        computation_ns: timebase.nanoseconds(u64::from(applied.computation)),
        constraint_ns: timebase.nanoseconds(u64::from(applied.constraint)),
    })
}

/// A thread running PCM work in the realtime class, which steps out of it for control
/// work (property queries, native object teardown, lease refreshes) so a realtime thread
/// never blocks on or spends its budget for non-audio operations.
#[derive(Debug)]
pub struct ScheduledThread<S> {
    scheduler: S,
    budget: Cell<(Duration, Duration, Duration)>,
    applied: Cell<RealtimePolicy>,
    _thread: PhantomData<*const ()>,
}

/// A scheduling change that failed around control work.
#[derive(Debug)]
pub enum Transition {
    /// The thread stayed realtime while the control work ran.
    Demote(io::Error),
    /// The thread stays at standard priority until a later re-entry succeeds.
    Promote(io::Error),
}

impl<S: Scheduling> ScheduledThread<S> {
    /// Promotes the calling thread through the supplied scheduling port.
    ///
    /// # Errors
    /// Returns the promotion error. The thread is then left at standard priority, even
    /// if the kernel applied the request but it could not be read back.
    pub fn with_scheduler(
        scheduler: S,
        period: Duration,
        computation: Duration,
        constraint: Duration,
    ) -> io::Result<Self> {
        match scheduler.promote(period, computation, constraint) {
            Ok(applied) => Ok(Self {
                scheduler,
                budget: Cell::new((period, computation, constraint)),
                applied: Cell::new(applied),
                _thread: PhantomData,
            }),
            Err(error) => {
                let _ = scheduler.demote();
                Err(error)
            }
        }
    }

    #[must_use]
    pub fn applied(&self) -> RealtimePolicy {
        self.applied.get()
    }

    /// Changes the budget; it takes effect when the thread next re-enters the class.
    pub fn retune(&self, period: Duration, computation: Duration, constraint: Duration) {
        self.budget.set((period, computation, constraint));
    }

    /// Runs control work at standard priority, then re-enters the realtime class.
    /// The work runs even when demotion fails, since control work cannot be skipped;
    /// the failure is reported so the owner can treat the thread as degraded.
    pub fn outside<R>(&self, work: impl FnOnce() -> R) -> (R, Result<(), Transition>) {
        let demoted = self.scheduler.demote();
        let result = work();
        (result, self.restore(demoted))
    }

    /// Re-enters the realtime class after control work, unless demotion had failed.
    pub(crate) fn restore(&self, demoted: io::Result<()>) -> Result<(), Transition> {
        demoted.map_err(Transition::Demote)?;
        let (period, computation, constraint) = self.budget.get();
        self.scheduler
            .promote(period, computation, constraint)
            .map(|applied| self.applied.set(applied))
            .map_err(|error| {
                // A request applied but not read back would otherwise stay realtime.
                let _ = self.scheduler.demote();
                Transition::Promote(error)
            })
    }
}

/// Delays in power-of-two microsecond buckets, recorded without allocation or locks by
/// the one thread that owns it; read and reset by that thread's control step.
#[derive(Debug, Default, Clone)]
pub struct LatencyHistogram {
    buckets: [u64; LATENCY_BUCKETS],
    maximum_ns: u64,
}

const LATENCY_BUCKETS: usize = 24;

/// A window's summary: each percentile is its bucket's upper bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencySummary {
    pub count: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub maximum_us: u64,
}

impl LatencyHistogram {
    pub fn record(&mut self, nanoseconds: u64) {
        let microseconds = nanoseconds / 1_000;
        // Bucket b holds delays below 2^b µs; the last also holds everything longer.
        let bucket = (u64::BITS - microseconds.leading_zeros()) as usize;
        self.buckets[bucket.min(LATENCY_BUCKETS - 1)] += 1;
        self.maximum_ns = self.maximum_ns.max(nanoseconds);
    }

    /// Summarizes and clears the window; `None` when nothing was recorded.
    pub fn take(&mut self) -> Option<LatencySummary> {
        let window = std::mem::take(self);
        let count: u64 = window.buckets.iter().sum();
        if count == 0 {
            return None;
        }
        let at = |permille: u64| {
            let rank = (count * permille).div_ceil(1_000);
            let bucket = window
                .buckets
                .iter()
                .scan(0, |seen, &observed| {
                    *seen += observed;
                    Some(*seen)
                })
                .position(|seen| seen >= rank)
                .unwrap_or(LATENCY_BUCKETS - 1);
            1_u64 << bucket
        };
        Some(LatencySummary {
            count,
            p50_us: at(500),
            p99_us: at(990),
            p999_us: at(999),
            maximum_us: window.maximum_ns / 1_000,
        })
    }
}

#[cfg(test)]
mod tests;
