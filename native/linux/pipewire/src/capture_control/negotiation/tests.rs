use super::*;
use mixrelay_time::{ManualClock, MonotonicClock};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
struct Configuration {
    monitoring: bool,
    channels: u16,
}

struct Host {
    snapshots: RefCell<VecDeque<(u64, Option<Configuration>)>>,
    current: RefCell<(u64, Option<Configuration>)>,
    actions: RefCell<Vec<&'static str>>,
    rejected: Cell<bool>,
    removed: Cell<bool>,
}
impl Host {
    fn new(snapshots: impl IntoIterator<Item = (u64, Option<Configuration>)>) -> Self {
        Self {
            snapshots: RefCell::new(snapshots.into_iter().collect()),
            current: RefCell::new((0, None)),
            actions: RefCell::new(Vec::new()),
            rejected: Cell::new(false),
            removed: Cell::new(false),
        }
    }
}
impl CaptureHost for Host {
    type Configuration = Configuration;
    fn snapshot(&self) -> Result<Snapshot<Configuration>, PreparationError> {
        if self.removed.get() {
            return Err(PreparationError::Host("removed".into()));
        }
        if let Some(snapshot) = self.snapshots.borrow_mut().pop_front() {
            *self.current.borrow_mut() = snapshot;
        }
        let (generation, configured) = self.current.borrow().clone();
        Ok(Snapshot {
            generation,
            configured,
        })
    }
    fn request(&self) -> Result<(), PreparationError> {
        self.actions.borrow_mut().push("request");
        Ok(())
    }
    fn monitoring(configuration: &Configuration) -> bool {
        configuration.monitoring
    }
    fn enable_monitor(
        &self,
        mut configuration: Configuration,
        rate: u32,
    ) -> Result<(Configuration, u64), PreparationError> {
        assert_eq!(rate, 48_000);
        self.actions.borrow_mut().push("write");
        if self.rejected.get() {
            return Err(PreparationError::Host("setter rejected".into()));
        }
        configuration.monitoring = true;
        Ok((configuration, self.current.borrow().0))
    }
    fn timeout(&self) -> PreparationError {
        PreparationError::Host("timeout".into())
    }
}
fn config(monitoring: bool) -> Configuration {
    Configuration {
        monitoring,
        channels: 2,
    }
}

#[test]
fn already_monitored_manager_configuration_needs_no_setter_or_wait() {
    let host = Host::new([(7, Some(config(true)))]);
    let clock = ManualClock::default();
    prepare(&host, &clock, 48_000).unwrap();
    assert!(host.actions.borrow().is_empty());
    assert_eq!(
        clock.now(),
        mixrelay_time::Instant::from_origin(Duration::ZERO)
    );
}

#[test]
fn monitor_readback_requires_a_new_generation_and_exact_configuration() {
    let host = Host::new([
        (7, None),
        (7, Some(config(false))),
        (7, Some(config(true))),
        (7, Some(config(true))),
        (8, None),
        (8, Some(config(true))),
    ]);
    let clock = ManualClock::default();
    prepare(&host, &clock, 48_000).unwrap();
    assert_eq!(*host.actions.borrow(), ["request", "write", "request"]);
    assert!(
        clock
            .now()
            .duration_since(mixrelay_time::Instant::from_origin(Duration::ZERO))
            >= Duration::from_millis(8)
    );
}

#[test]
fn unchanged_generation_times_out_without_repeating_requests() {
    let host = Host::new([(7, None)]);
    let clock = ManualClock::default();
    let error = prepare(&host, &clock, 48_000).unwrap_err();
    assert!(error.to_string().contains("timeout"));
    assert_eq!(*host.actions.borrow(), ["request"]);
    assert_eq!(
        clock
            .now()
            .duration_since(mixrelay_time::Instant::from_origin(Duration::ZERO)),
        PREPARATION_TIMEOUT
    );
}

#[test]
fn setter_failure_removal_and_mismatched_monitor_readback_abort_preparation() {
    let host = Host::new([(7, Some(config(false)))]);
    host.rejected.set(true);
    assert!(
        prepare(&host, &ManualClock::default(), 48_000)
            .unwrap_err()
            .to_string()
            .contains("setter rejected")
    );
    assert_eq!(*host.actions.borrow(), ["write"]);
    host.removed.set(true);
    assert!(
        prepare(&host, &ManualClock::default(), 48_000)
            .unwrap_err()
            .to_string()
            .contains("removed")
    );
    assert_eq!(*host.actions.borrow(), ["write"]);

    let host = Host::new([
        (7, Some(config(false))),
        (
            8,
            Some(Configuration {
                monitoring: true,
                channels: 1,
            }),
        ),
    ]);
    assert!(matches!(
        prepare(&host, &ManualClock::default(), 48_000),
        Err(PreparationError::ReadbackMismatch)
    ));
}
