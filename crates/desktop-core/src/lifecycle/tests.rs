use super::*;
use mixrelay_time::{Instant, ManualClock, MonotonicClock};
use std::cell::Cell;
struct Timer(ManualClock);
impl MonotonicClock for Timer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl AsyncDelay for Timer {
    async fn wait_until(&self, deadline: Deadline) {
        self.0.advance(deadline.remaining(self.now()));
    }
}

#[tokio::test]
async fn failed_recovery_retries_and_stop_precedes_an_observed_exit() {
    let timer = Timer(ManualClock::default());
    let stopped = Cell::new(false);
    let polls = Cell::new(0);
    let replacements = Cell::new(0);
    let running = Cell::new(false);
    let failures = Cell::new(0);
    supervise(
        &timer,
        || stopped.get(),
        || {
            polls.set(polls.get() + 1);
            if polls.get() == 1 {
                return Err("observation failed");
            }
            if running.get() {
                running.set(false);
                stopped.set(true);
            }
            Ok(true)
        },
        || {
            replacements.set(replacements.get() + 1);
            std::future::ready(if replacements.get() == 1 {
                Err("spawn failed")
            } else {
                running.set(true);
                Ok(())
            })
        },
        |_| failures.set(failures.get() + 1),
    )
    .await;
    assert_eq!(polls.get(), 4);
    assert_eq!(replacements.get(), 2);
    assert_eq!(failures.get(), 2);
    assert!(!running.get());
}

#[tokio::test]
async fn slow_native_exit_keeps_waiting_after_its_one_notice() {
    let timer = Timer(ManualClock::default());
    let start = timer.now();
    let mut notices = 0;
    let record = await_engine_exit(
        &timer,
        async || {
            Ok::<_, std::io::Error>(
                if timer.now().duration_since(start) < Duration::from_secs(65) {
                    ExitObservation::Pending
                } else {
                    ExitObservation::Stopped("stopped")
                },
            )
        },
        || notices += 1,
    )
    .await
    .unwrap();
    assert_eq!(record, "stopped");
    assert_eq!(notices, 1);
    assert_eq!(timer.now().duration_since(start), Duration::from_secs(65));
}

#[tokio::test]
async fn inaccessible_lease_never_reports_a_stopped_engine() {
    let result = await_engine_exit(
        &Timer(ManualClock::default()),
        async || Err::<ExitObservation<()>, _>(std::io::Error::other("lease inaccessible")),
        || panic!("immediate error needs no notice"),
    )
    .await;
    assert_eq!(result.unwrap_err().to_string(), "lease inaccessible");
}

#[test]
fn a_lost_engine_is_reported_once_woken_every_second_check_and_recovered() {
    let mut absence = Absence::default();
    let seen: Vec<_> = [
        false, false, true, false, false, false, false, true, true, false, true,
    ]
    .into_iter()
    .map(|owned| absence.observe(owned))
    .collect();
    assert_eq!(
        seen,
        [
            Watch::Lost { unexpected: false },
            Watch::Wake,
            Watch::Recovered,
            Watch::Lost { unexpected: true },
            Watch::Wake,
            Watch::Waiting,
            Watch::Wake,
            Watch::Recovered,
            Watch::Running,
            Watch::Lost { unexpected: true },
            Watch::Recovered
        ]
    );
}

#[test]
fn native_registration_is_demand_started_without_autorestart() {
    let mut config = serde_json::json!({"Label":"io.mixrelay.audio.engine", "KeepAlive":false,
        "RunAtLoad":false, "MachServices":{"io.mixrelay.audio.engine":true}});
    assert!(is_app_owned_configuration(&config));
    config["KeepAlive"] = true.into();
    assert!(!is_app_owned_configuration(&config));
    config["KeepAlive"] = false.into();
    config["RunAtLoad"] = true.into();
    assert!(!is_app_owned_configuration(&config));
    config["RunAtLoad"] = false.into();
    config["MachServices"] = serde_json::Value::Null;
    assert!(!is_app_owned_configuration(&config));
}
