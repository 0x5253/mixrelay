//! Explicit connection, readiness, and bounded controller launch policy.
use crate::timing::{AsyncDelay, before};
use mixrelay_time::Deadline;
use serde::Serialize;
use serde_json::Value;
use std::{future::Future, time::Duration};

// Cold launch may race the previous process finishing; only reaped exits allow retry.
const LAUNCH_LIMIT: u8 = 3;
const RETRY_INTERVAL: Duration = Duration::from_secs(1);
const PROBE_INTERVAL: Duration = Duration::from_millis(200);

/// The editor-visible readiness of the installed runtime and its owned engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Ready,
    NotInstalled,
    Incompatible,
    Unavailable,
}

/// Progress of an explicit user connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectStage {
    Checking,
    Starting,
    PreparingEngine,
}

/// Facts read from a runtime without starting it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Silent,
    Incompatible,
    EngineStarting,
    Ready,
}
impl Probe {
    /// Projects probe facts to the editor's readiness contract.
    #[must_use]
    pub fn status(self) -> RuntimeStatus {
        match self {
            Self::Silent | Self::EngineStarting => RuntimeStatus::Unavailable,
            Self::Incompatible => RuntimeStatus::Incompatible,
            Self::Ready => RuntimeStatus::Ready,
        }
    }
}

/// The expected editor/runtime wire and installed product versions.
pub struct Compatibility<'a> {
    pub protocol: u64,
    pub version: &'a str,
}
impl Compatibility<'_> {
    /// Accepts only the Driver runtime shipped with this editor.
    #[must_use]
    pub fn accepts(&self, hello: &Value) -> bool {
        hello["protocolVersion"] == self.protocol
            && hello["product"] == "mixrelay-driver"
            && hello["version"] == self.version
    }
}

/// Read-only communication with one runtime and its owned engine.
pub trait RuntimeEndpoint {
    /// Returns the runtime identity, or no observation when transport fails.
    fn hello(&self) -> impl Future<Output = Option<Value>> + Send;
    /// Whether the runtime's owned engine has acknowledged readiness.
    fn engine_ready(&self) -> impl Future<Output = bool> + Send;
}

/// Observation of a child launched by this connection attempt.
pub trait LaunchObservation {
    /// True only after the launched process has actually been reaped.
    fn exited(&self) -> bool;
}

/// Native admission and creation of a detached controller process.
pub trait ControllerLauncher {
    type Launch: LaunchObservation;
    /// Whether both controller and engine ownership admit a fresh controller.
    /// # Errors
    /// Returns native ownership observation failure.
    fn admission_available(&self) -> std::io::Result<bool>;
    /// Starts a controller whose process lifetime is independent of the editor.
    /// # Errors
    /// Returns launch or reaper preparation failure.
    fn launch(&self) -> std::io::Result<Self::Launch>;
}

/// Probes without launching; engine preparation is reported as soon as the runtime answers.
pub async fn probe(
    endpoint: &impl RuntimeEndpoint,
    compatibility: &Compatibility<'_>,
    timer: &impl AsyncDelay,
    exchange_timeout: Duration,
    engine_pending: impl FnOnce(),
) -> Probe {
    let hello_deadline = Deadline::after(timer.now(), exchange_timeout);
    let Some(Some(hello)) = before(timer, hello_deadline, endpoint.hello()).await else {
        return Probe::Silent;
    };
    if !compatibility.accepts(&hello) {
        return Probe::Incompatible;
    }
    engine_pending();
    let deadline = Deadline::after(timer.now(), exchange_timeout);
    if before(timer, deadline, endpoint.engine_ready()).await == Some(true) {
        Probe::Ready
    } else {
        Probe::EngineStarting
    }
}

/// Compatibility and time budgets of one explicit editor connection.
pub struct ConnectOptions<'a> {
    pub compatibility: Compatibility<'a>,
    pub timeout: Duration,
    pub exchange_timeout: Duration,
}

/// Connects explicitly, limiting launch retries and preserving live owner authority.
/// # Errors
/// Returns native admission or launch failure; readiness expiry is `Unavailable`.
pub async fn connect_until_ready<L: ControllerLauncher>(
    endpoint: &impl RuntimeEndpoint,
    launcher: &L,
    timer: &impl AsyncDelay,
    options: ConnectOptions<'_>,
    mut report: impl FnMut(ConnectStage),
) -> std::io::Result<RuntimeStatus> {
    let deadline = Deadline::after(timer.now(), options.timeout);
    let mut child: Option<L::Launch> = None;
    let mut attempts = 0;
    let mut next_launch = timer.now();
    let mut reported = ConnectStage::Checking;
    let mut advance = |stage| {
        if stage != reported {
            reported = stage;
            report(stage);
        }
    };
    loop {
        let checked = probe(
            endpoint,
            &options.compatibility,
            timer,
            options.exchange_timeout,
            || advance(ConnectStage::PreparingEngine),
        );
        match before(timer, deadline, checked).await {
            Some(probe @ (Probe::Ready | Probe::Incompatible)) => return Ok(probe.status()),
            Some(Probe::EngineStarting | Probe::Silent) => {}
            None => return Ok(RuntimeStatus::Unavailable),
        }
        if deadline.has_passed(timer.now()) {
            return Ok(RuntimeStatus::Unavailable);
        }
        if child.as_ref().is_none_or(LaunchObservation::exited) && timer.now() >= next_launch {
            if attempts == LAUNCH_LIMIT {
                return Ok(RuntimeStatus::Unavailable);
            }
            let Some(hello) = before(timer, deadline, endpoint.hello()).await else {
                return Ok(RuntimeStatus::Unavailable);
            };
            let running = hello.is_some_and(|hello| options.compatibility.accepts(&hello));
            if !running && launcher.admission_available()? {
                advance(ConnectStage::Starting);
                child = Some(launcher.launch()?);
                attempts += 1;
                next_launch = timer.now().saturating_add(RETRY_INTERVAL);
            }
        }
        timer
            .wait_until(Deadline::at(
                deadline
                    .instant()
                    .min(timer.now().saturating_add(PROBE_INTERVAL)),
            ))
            .await;
    }
}

#[cfg(test)]
mod tests;
