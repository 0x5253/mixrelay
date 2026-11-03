//! Generation-bound capture negotiation using decoded snapshots and explicit time.
use crate::control_time::{ControlTime, PREPARATION_TIMEOUT, poll};
use mixrelay_time::Deadline;

#[derive(Debug, thiserror::Error)]
pub(super) enum PreparationError {
    #[error("capture preparation: {0}")]
    Host(String),
    #[error("capture monitor PortConfig readback differs")]
    ReadbackMismatch,
}

pub(super) struct Snapshot<C> {
    pub(super) generation: u64,
    pub(super) configured: Option<C>,
}

pub(super) trait CaptureHost {
    type Configuration: PartialEq;
    fn snapshot(&self) -> Result<Snapshot<Self::Configuration>, PreparationError>;
    fn request(&self) -> Result<(), PreparationError>;
    fn monitoring(configuration: &Self::Configuration) -> bool;
    fn enable_monitor(
        &self,
        configuration: Self::Configuration,
        rate: u32,
    ) -> Result<(Self::Configuration, u64), PreparationError>;
    fn timeout(&self) -> PreparationError;
}

pub(super) fn prepare<H: CaptureHost>(
    host: &H,
    time: &dyn ControlTime,
    rate: u32,
) -> Result<(), PreparationError> {
    let deadline = Deadline::after(time.now(), PREPARATION_TIMEOUT);
    let configuration = wait_config(host, time, None, deadline)?;
    if H::monitoring(&configuration) {
        return Ok(());
    }
    let (wanted, generation) = host.enable_monitor(configuration, rate)?;
    let actual = wait_config(host, time, Some(generation), deadline)?;
    if actual != wanted {
        return Err(PreparationError::ReadbackMismatch);
    }
    Ok(())
}

fn wait_config<H: CaptureHost>(
    host: &H,
    time: &dyn ControlTime,
    after: Option<u64>,
    deadline: Deadline,
) -> Result<H::Configuration, PreparationError> {
    let mut requested = None;
    loop {
        let snapshot = host.snapshot()?;
        let changed = after.is_none_or(|previous| snapshot.generation != previous);
        if changed && let Some(configuration) = snapshot.configured {
            return Ok(configuration);
        }
        if changed && requested != Some(snapshot.generation) {
            host.request()?;
            requested = Some(snapshot.generation);
        }
        if deadline.has_passed(time.now()) {
            return Err(host.timeout());
        }
        poll(time, deadline);
    }
}

#[cfg(test)]
mod tests;
