//! Controller-owned recovery cadence and native-exit observation policy.
use crate::timing::AsyncDelay;
use mixrelay_time::Deadline;
use std::{future::Future, time::Duration};

pub mod shutdown;

#[derive(Debug, PartialEq, Eq)]
pub enum Watch {
    Running,
    /// The engine is absent; reported once per outage. Captures are released even
    /// before the engine was first seen, since one may have died between checks.
    Lost {
        unexpected: bool,
    },
    Waiting,
    Wake,
    Recovered,
}

/// Wakes on every second absent check, so an engine still starting is left alone. An engine
/// not yet seen running is not reported as an unexpected stop.
#[derive(Default)]
pub struct Absence {
    checks: u8,
    seen: bool,
    lost: bool,
}

impl Absence {
    pub fn observe(&mut self, owned: bool) -> Watch {
        if owned {
            self.checks = 0;
            self.seen = true;
            return if std::mem::take(&mut self.lost) {
                Watch::Recovered
            } else {
                Watch::Running
            };
        }
        self.checks += 1;
        if !std::mem::replace(&mut self.lost, true) {
            return Watch::Lost {
                unexpected: self.seen,
            };
        }
        if self.checks < 2 {
            return Watch::Waiting;
        }
        self.checks = 0;
        Watch::Wake
    }
}

/// The controller's view of an engine the OS starts on demand and never restarts itself.
pub trait OnDemandEngine {
    type Error;
    /// The controller confirmed its shutdown; the watch ends.
    fn shutdown_finished(&self) -> bool;
    /// A stop was requested; the engine is no longer wanted, even if teardown is retried.
    fn stop_requested(&self) -> bool;
    /// # Errors
    /// Returns why the engine's state could not be observed.
    fn is_running(&self) -> Result<bool, Self::Error>;
    fn observation_failed(&self, error: Self::Error);
    /// The engine is gone; reported once per outage.
    fn lost(&self, unexpected: bool) -> impl Future<Output = ()> + Send;
    fn wake(&self);
    /// The engine runs again after an outage.
    fn recovered(&self);
}

/// Checks the engine once a second while the controller wants it running. An observation
/// failure is never taken as an absent engine.
pub async fn watch_on_demand_engine<E: OnDemandEngine>(timer: &impl AsyncDelay, engine: &E) {
    let mut absence = Absence::default();
    loop {
        timer
            .wait_until(Deadline::after(timer.now(), Duration::from_secs(1)))
            .await;
        if engine.shutdown_finished() {
            return;
        }
        if engine.stop_requested() {
            continue;
        }
        let running = match engine.is_running() {
            Ok(running) => running,
            Err(error) => {
                engine.observation_failed(error);
                continue;
            }
        };
        match absence.observe(running) {
            Watch::Running | Watch::Waiting => {}
            Watch::Lost { unexpected } => engine.lost(unexpected).await,
            Watch::Wake => engine.wake(),
            Watch::Recovered => engine.recovered(),
        }
    }
}

/// Runs `attempt` until it succeeds, at most `attempts` times, waiting `interval` after
/// each failure. Returns whether an attempt succeeded.
///
/// Like [`await_engine_exit`] and [`supervise`], this takes a closure returning a future
/// rather than an async closure, so a spawned caller's future can be proven `Send`.
pub async fn retry<R: Future<Output = bool>>(
    timer: &impl AsyncDelay,
    attempts: u32,
    interval: Duration,
    mut attempt: impl FnMut() -> R,
) -> bool {
    for _ in 0..attempts {
        if attempt().await {
            return true;
        }
        timer
            .wait_until(Deadline::after(timer.now(), interval))
            .await;
    }
    false
}

/// Native teardown keeps ownership until both the lease and service socket have ended.
pub enum ExitObservation<T> {
    Pending,
    Stopped(T),
}

/// One delayed notice while an acknowledged shutdown continues waiting.
pub struct ExitNotice {
    at: Deadline,
    notified: bool,
}
impl ExitNotice {
    /// Native audio teardown may be slow; notify once after thirty seconds.
    #[must_use]
    pub fn new(timer: &impl AsyncDelay) -> Self {
        Self {
            at: Deadline::after(timer.now(), Duration::from_secs(30)),
            notified: false,
        }
    }
    /// Whether the caller should publish its one pending-exit notice now.
    pub fn due(&mut self, timer: &impl AsyncDelay) -> bool {
        if self.notified || !self.at.has_passed(timer.now()) {
            return false;
        }
        self.notified = true;
        true
    }
}

/// Waits for confirmed native teardown; observation failures preserve pending ownership.
/// # Errors
/// Returns an observation failure while keeping native exit unconfirmed.
pub async fn await_engine_exit<T, E, R: Future<Output = Result<ExitObservation<T>, E>>>(
    timer: &impl AsyncDelay,
    mut observe: impl FnMut() -> R,
    mut waiting: impl FnMut(),
) -> Result<T, E> {
    let mut notice = ExitNotice::new(timer);
    loop {
        if let ExitObservation::Stopped(record) = observe().await? {
            return Ok(record);
        }
        if notice.due(timer) {
            waiting();
        }
        timer
            .wait_until(Deadline::after(timer.now(), Duration::from_millis(100)))
            .await;
    }
}

/// Failures observed while supervising one controller-owned engine.
pub enum SupervisionFailure<'a, E> {
    Observation(&'a E),
    Recovery(&'a E),
}

/// Retries replacement only while the same controller still wants its engine running.
pub async fn supervise<E, R: Future<Output = Result<(), E>>>(
    timer: &impl AsyncDelay,
    stopping: impl Fn() -> bool,
    mut exited: impl FnMut() -> Result<bool, E>,
    mut recover: impl FnMut() -> R,
    mut failure: impl FnMut(SupervisionFailure<'_, E>),
) {
    loop {
        timer
            .wait_until(Deadline::after(timer.now(), Duration::from_millis(500)))
            .await;
        if stopping() {
            return;
        }
        match exited() {
            Ok(false) => {}
            Ok(true) => {
                if stopping() {
                    return;
                }
                if let Err(error) = recover().await {
                    failure(SupervisionFailure::Recovery(&error));
                }
            }
            Err(error) => failure(SupervisionFailure::Observation(&error)),
        }
    }
}

/// Validates the native engine's demand-start ownership contract.
#[must_use]
pub fn is_app_owned_configuration(config: &serde_json::Value) -> bool {
    config["Label"] == "io.mixrelay.audio.engine"
        && config["KeepAlive"] == false
        && config["RunAtLoad"] == false
        && config["MachServices"]["io.mixrelay.audio.engine"] == true
}

#[cfg(test)]
mod tests;
