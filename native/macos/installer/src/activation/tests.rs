use super::*;
use mixrelay_time::{Instant, ManualClock};
use std::collections::VecDeque;

fn observation() -> DriverObservation {
    DriverObservation {
        state: DriverState::Ready,
        installed_version: Some("0.1.0".into()),
        owner_process_id: Some(42),
        registered_pid: Some(42),
    }
}

struct Probe {
    observations: VecDeque<Result<DriverObservation, &'static str>>,
    calls: usize,
}

impl DriverReadiness for Probe {
    type Error = &'static str;
    fn observe(&mut self) -> Result<DriverObservation, Self::Error> {
        self.calls += 1;
        self.observations
            .pop_front()
            .unwrap_or(Err("not available"))
    }
}

#[test]
fn readiness_requires_current_owner_and_committed_version() {
    let mut observed = observation();
    assert!(validate::<()>(&observed, "0.1.0").is_ok());
    for state in [
        DriverState::InstalledNotLoaded,
        DriverState::NotInstalled,
        DriverState::Unavailable,
    ] {
        observed.state = state;
        assert!(matches!(
            validate::<()>(&observed, "0.1.0"),
            Err(ReadinessError::NotReady(_))
        ));
    }
    observed.state = DriverState::Ready;
    assert!(matches!(
        validate::<()>(&observed, "0.2.0"),
        Err(ReadinessError::VersionMismatch)
    ));
    observed.installed_version = None;
    assert!(matches!(
        validate::<()>(&observed, "0.1.0"),
        Err(ReadinessError::VersionMismatch)
    ));
    observed.installed_version = Some("0.1.0".into());
    for owner in [None, Some(0), Some(41)] {
        observed.owner_process_id = owner;
        assert!(matches!(
            validate::<()>(&observed, "0.1.0"),
            Err(ReadinessError::OwnerMismatch)
        ));
    }
    observed.owner_process_id = Some(42);
    for pid in [None, Some(0), Some(43)] {
        observed.registered_pid = pid;
        assert!(validate::<()>(&observed, "0.1.0").is_err());
    }
}

#[test]
fn retries_reobserve_both_response_and_owner_until_they_agree() {
    let mut stale = observation();
    stale.registered_pid = Some(43);
    let mut probe = Probe {
        observations: VecDeque::from([Err("socket not ready"), Ok(stale), Ok(observation())]),
        calls: 0,
    };
    let clock = ManualClock::default();
    wait_until_ready("0.1.0", &mut probe, &clock).unwrap();
    assert_eq!(probe.calls, 3);
    assert_eq!(
        clock.now(),
        Instant::from_origin(Duration::from_millis(500))
    );
}

#[test]
fn deadline_returns_the_last_failure_without_waiting_past_the_budget() {
    let mut probe = Probe {
        observations: VecDeque::new(),
        calls: 0,
    };
    let clock = ManualClock::default();
    assert!(matches!(
        wait_until_ready("0.1.0", &mut probe, &clock),
        Err(ReadinessError::Observation("not available"))
    ));
    assert_eq!(clock.now(), Instant::from_origin(Duration::from_secs(30)));
    assert_eq!(probe.calls, 121);
}
