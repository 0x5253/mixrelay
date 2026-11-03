//! Bounded, main-thread CLAP timer scheduling for one plugin instance.

use clap_sys::id::{CLAP_INVALID_ID, clap_id};
use std::time::{Duration, Instant};

pub(crate) const MAX_TIMERS: usize = 32;
const MIN_PERIOD_MS: u32 = 33;

#[derive(Clone, Copy)]
struct TimerEntry {
    id: Option<clap_id>,
    period: Duration,
    next_deadline: Option<Instant>,
}

impl TimerEntry {
    const EMPTY: Self = Self {
        id: None,
        period: Duration::ZERO,
        next_deadline: None,
    };
}

#[derive(Clone, Copy)]
pub(crate) struct TimerRegistry {
    entries: [TimerEntry; MAX_TIMERS],
    next_id: clap_id,
}

impl Default for TimerRegistry {
    fn default() -> Self {
        Self {
            entries: [TimerEntry::EMPTY; MAX_TIMERS],
            next_id: 0,
        }
    }
}

impl TimerRegistry {
    pub(crate) fn register(&mut self, period_ms: u32, now: Instant) -> Option<clap_id> {
        if period_ms == 0 || self.next_id == CLAP_INVALID_ID {
            return None;
        }
        let entry = self.entries.iter_mut().find(|entry| entry.id.is_none())?;
        let period = Duration::from_millis(u64::from(period_ms.max(MIN_PERIOD_MS)));
        let next_deadline = now.checked_add(period)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).unwrap_or(CLAP_INVALID_ID);
        *entry = TimerEntry {
            id: Some(id),
            period,
            next_deadline: Some(next_deadline),
        };
        Some(id)
    }

    pub(crate) fn unregister(&mut self, id: clap_id) -> bool {
        if id == CLAP_INVALID_ID {
            return false;
        }
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == Some(id)) else {
            return false;
        };
        *entry = TimerEntry::EMPTY;
        true
    }

    pub(crate) fn is_registered(&self, id: clap_id) -> bool {
        self.entries.iter().any(|entry| entry.id == Some(id))
    }

    pub(crate) fn clear(&mut self) {
        self.entries.fill(TimerEntry::EMPTY);
    }

    pub(crate) fn collect_due(&mut self, now: Instant) -> DueTimers {
        let mut due = DueTimers::default();
        for entry in &mut self.entries {
            let (Some(id), Some(deadline)) = (entry.id, entry.next_deadline) else {
                continue;
            };
            if now < deadline {
                continue;
            }
            let Some(next_deadline) = now.checked_add(entry.period) else {
                *entry = TimerEntry::EMPTY;
                continue;
            };
            entry.next_deadline = Some(next_deadline);
            due.push(id);
        }
        due
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DueTimers {
    ids: [clap_id; MAX_TIMERS],
    len: usize,
}

impl Default for DueTimers {
    fn default() -> Self {
        Self {
            ids: [CLAP_INVALID_ID; MAX_TIMERS],
            len: 0,
        }
    }
}

impl DueTimers {
    fn push(&mut self, id: clap_id) {
        self.ids[self.len] = id;
        self.len += 1;
    }

    pub(crate) fn as_slice(&self) -> &[clap_id] {
        &self.ids[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_bounded_rejects_invalid_periods_and_never_reuses_ids() {
        let now = Instant::now();
        let mut registry = TimerRegistry::default();
        assert_eq!(registry.register(0, now), None);

        let ids = (0..MAX_TIMERS)
            .map(|_| registry.register(1, now).expect("available timer slot"))
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), MAX_TIMERS);
        assert!(ids.iter().copied().all(|id| id != CLAP_INVALID_ID));
        assert_eq!(registry.register(33, now), None);

        assert!(registry.unregister(ids[0]));
        assert!(!registry.unregister(ids[0]));
        assert!(!registry.unregister(CLAP_INVALID_ID));
        assert!(!registry.unregister(CLAP_INVALID_ID - 1));

        registry.clear();
        assert_eq!(registry.register(33, now), Some(32));
    }

    #[test]
    fn due_timers_wait_for_the_period_and_skip_missed_intervals() {
        let start = Instant::now();
        let mut registry = TimerRegistry::default();
        let id = registry.register(1, start).expect("timer registration");

        assert!(
            registry
                .collect_due(start + Duration::from_millis(32))
                .as_slice()
                .is_empty()
        );
        let delayed_tick = start + Duration::from_secs(10);
        assert_eq!(registry.collect_due(delayed_tick).as_slice(), &[id]);
        assert!(registry.collect_due(delayed_tick).as_slice().is_empty());
        assert!(
            registry
                .collect_due(delayed_tick + Duration::from_millis(32))
                .as_slice()
                .is_empty()
        );
        assert_eq!(
            registry
                .collect_due(delayed_tick + Duration::from_millis(33))
                .as_slice(),
            &[id]
        );
    }

    #[test]
    fn clearing_lifecycle_state_cancels_every_scheduled_timer() {
        let now = Instant::now();
        let mut registry = TimerRegistry::default();
        let first = registry.register(33, now).expect("first timer");
        let second = registry.register(33, now).expect("second timer");
        registry.clear();

        assert!(!registry.is_registered(first));
        assert!(!registry.is_registered(second));
        assert!(
            registry
                .collect_due(now + Duration::from_secs(1))
                .as_slice()
                .is_empty()
        );
    }
}
