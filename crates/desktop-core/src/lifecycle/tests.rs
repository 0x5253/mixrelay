use super::*;
use mixrelay_time::{Instant, ManualClock, MonotonicClock};
use std::{cell::Cell, collections::VecDeque, sync::Mutex};
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
        || {
            std::future::ready(Ok::<_, std::io::Error>(
                if timer.now().duration_since(start) < Duration::from_secs(65) {
                    ExitObservation::Pending
                } else {
                    ExitObservation::Stopped("stopped")
                },
            ))
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
        || {
            std::future::ready(Err::<ExitObservation<()>, _>(std::io::Error::other(
                "lease inaccessible",
            )))
        },
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

#[derive(Clone, Copy)]
enum Tick {
    Observe(Result<bool, &'static str>),
    Stopping,
    Finished,
}

#[derive(Debug, PartialEq, Eq)]
enum Seen {
    ObservationFailed(&'static str),
    Lost { unexpected: bool },
    Wake,
    Recovered,
}

/// Replays one scripted tick per watch check and records what the watch did.
struct ScriptedEngine {
    ticks: Mutex<VecDeque<Tick>>,
    current: Mutex<Option<Tick>>,
    seen: Mutex<Vec<Seen>>,
}

impl ScriptedEngine {
    fn new(ticks: impl IntoIterator<Item = Tick>) -> Self {
        Self {
            ticks: Mutex::new(ticks.into_iter().collect()),
            current: Mutex::default(),
            seen: Mutex::default(),
        }
    }

    fn current(&self) -> Tick {
        self.current
            .lock()
            .unwrap()
            .expect("checked before observing")
    }

    fn record(&self, seen: Seen) {
        self.seen.lock().unwrap().push(seen);
    }

    fn seen(&self) -> Vec<Seen> {
        std::mem::take(&mut self.seen.lock().unwrap())
    }
}

impl OnDemandEngine for ScriptedEngine {
    type Error = &'static str;
    fn shutdown_finished(&self) -> bool {
        let tick = self
            .ticks
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script ends with Finished");
        *self.current.lock().unwrap() = Some(tick);
        matches!(tick, Tick::Finished)
    }
    fn stop_requested(&self) -> bool {
        matches!(self.current(), Tick::Stopping)
    }
    fn is_running(&self) -> Result<bool, Self::Error> {
        match self.current() {
            Tick::Observe(running) => running,
            Tick::Stopping | Tick::Finished => panic!("a stopping engine is not observed"),
        }
    }
    fn observation_failed(&self, error: Self::Error) {
        self.record(Seen::ObservationFailed(error));
    }
    async fn lost(&self, unexpected: bool) {
        self.record(Seen::Lost { unexpected });
    }
    fn wake(&self) {
        self.record(Seen::Wake);
    }
    fn recovered(&self) {
        self.record(Seen::Recovered);
    }
}

#[tokio::test]
async fn an_outage_releases_once_wakes_every_second_check_and_recovers() {
    let timer = Timer(ManualClock::default());
    let start = timer.now();
    let engine = ScriptedEngine::new([
        Tick::Observe(Ok(true)),
        Tick::Observe(Ok(false)),
        Tick::Observe(Ok(false)),
        Tick::Observe(Ok(false)),
        Tick::Observe(Ok(false)),
        Tick::Observe(Ok(true)),
        Tick::Finished,
    ]);
    watch_on_demand_engine(&timer, &engine).await;
    assert_eq!(
        engine.seen(),
        [
            Seen::Lost { unexpected: true },
            Seen::Wake,
            Seen::Wake,
            Seen::Recovered
        ]
    );
    assert_eq!(timer.now().duration_since(start), Duration::from_secs(7));
}

#[tokio::test]
async fn a_stop_request_leaves_a_lost_engine_stopped_until_shutdown_finishes() {
    let engine = ScriptedEngine::new([
        Tick::Observe(Ok(false)),
        Tick::Stopping,
        Tick::Stopping,
        Tick::Stopping,
        Tick::Finished,
    ]);
    watch_on_demand_engine(&Timer(ManualClock::default()), &engine).await;
    assert_eq!(engine.seen(), [Seen::Lost { unexpected: false }]);
}

#[tokio::test]
async fn an_unobservable_engine_is_never_taken_for_a_lost_one() {
    let engine = ScriptedEngine::new([
        Tick::Observe(Err("lease unreadable")),
        Tick::Observe(Err("lease unreadable")),
        Tick::Observe(Err("lease unreadable")),
        Tick::Finished,
    ]);
    watch_on_demand_engine(&Timer(ManualClock::default()), &engine).await;
    assert_eq!(
        engine.seen(),
        [
            Seen::ObservationFailed("lease unreadable"),
            Seen::ObservationFailed("lease unreadable"),
            Seen::ObservationFailed("lease unreadable"),
        ]
    );
}

#[tokio::test]
async fn retry_stops_at_the_first_success_and_waits_between_failures() {
    let timer = Timer(ManualClock::default());
    let start = timer.now();
    let mut attempts = 0;
    let succeeded = retry(&timer, 30, Duration::from_secs(1), || {
        attempts += 1;
        std::future::ready(attempts == 3)
    })
    .await;
    assert!(succeeded);
    assert_eq!(attempts, 3);
    assert_eq!(timer.now().duration_since(start), Duration::from_secs(2));
}

#[tokio::test]
async fn retry_gives_up_after_its_attempt_budget() {
    let timer = Timer(ManualClock::default());
    let mut attempts = 0;
    let succeeded = retry(&timer, 30, Duration::from_secs(1), || {
        attempts += 1;
        std::future::ready(false)
    })
    .await;
    assert!(!succeeded);
    assert_eq!(attempts, 30);
}
