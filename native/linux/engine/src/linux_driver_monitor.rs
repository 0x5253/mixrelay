//! Control-side graph-driver monitoring and failure classification after native drain.
use super::{clock, processing::LinuxProcessState};
use crate::processing_failure::{Failure, FailureMailbox};
use mixrelay_engine::session::AdapterRuntimeStatus;
use mixrelay_pipewire::{DriverProcessFailure, GraphDriver};
use std::{
    thread,
    time::{Duration, Instant},
};

pub(super) fn run(
    driver: GraphDriver<LinuxProcessState<'_>>,
    status: &AdapterRuntimeStatus,
    clock_diagnostics: &clock::DiscontinuityMailbox,
    failure: &FailureMailbox,
    resource_failures: &FailureMailbox,
) -> Result<(), Error> {
    let started = Instant::now();
    let mut last_clock_event = 0;
    let mut startup_timed_out = false;
    while !driver.ready()
        && !status.stop_requested()
        && failure.read().is_none()
        && driver.process_failure().is_none()
    {
        if driver.failed() {
            break;
        }
        if started.elapsed() > Duration::from_secs(5) {
            startup_timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    while !status.stop_requested()
        && !startup_timed_out
        && !driver.failed()
        && failure.read().is_none()
        && driver.process_failure().is_none()
    {
        // Native callbacks own audio scheduling; this wait only observes health and shutdown.
        report_clock_discontinuity(clock_diagnostics, &mut last_clock_event);
        report_resource_failure(resource_failures);
        thread::sleep(Duration::from_millis(20));
    }
    let disconnected = driver.failed();
    let native_failure = driver.finish(); // Drain before reading final RT diagnostics.
    report_resource_failure(resource_failures);
    report_clock_discontinuity(clock_diagnostics, &mut last_clock_event);
    Termination {
        startup_timed_out,
        disconnected,
        stop_requested: status.stop_requested(),
        processing_failure: failure.read(),
        native_failure,
    }
    .result()
}

/// Fixed terminal observations survive an unrelated shutdown request.
struct Termination {
    startup_timed_out: bool,
    disconnected: bool,
    stop_requested: bool,
    processing_failure: Option<Failure>,
    native_failure: Option<DriverProcessFailure>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "PipeWire graph failed: startup_timeout={startup_timed_out}, disconnected={disconnected}, processing={processing_failure:?}, native={native_failure:?}"
)]
pub struct Error {
    startup_timed_out: bool,
    disconnected: bool,
    processing_failure: Option<Failure>,
    native_failure: Option<DriverProcessFailure>,
}

impl Termination {
    fn result(self) -> Result<(), Error> {
        let disconnected = self.disconnected
            && !self.stop_requested
            && self.processing_failure.is_none()
            && self.native_failure.is_none();
        if self.startup_timed_out
            || disconnected
            || self.processing_failure.is_some()
            || self.native_failure.is_some()
        {
            return Err(Error {
                startup_timed_out: self.startup_timed_out,
                disconnected,
                processing_failure: self.processing_failure,
                native_failure: self.native_failure,
            });
        }
        Ok(())
    }
}

fn report_resource_failure(mailbox: &FailureMailbox) {
    if let Some((cause, native_code)) = mailbox.take() {
        tracing::warn!(
            ?cause,
            native_code,
            "PipeWire realtime resource operation rejected; retaining active graph"
        );
    }
}

fn report_clock_discontinuity(mailbox: &clock::DiscontinuityMailbox, last_event: &mut u64) {
    let Some(read) = mailbox.read_since(*last_event) else {
        return;
    };
    *last_event = read.event_number;
    let previous = read.event.previous;
    let current = read.event.current;
    tracing::warn!(
        event_number = read.event_number,
        overwritten = read.overwritten,
        previous_clock_id = previous.clock_id,
        previous_rate = previous.rate,
        previous_position = previous.position,
        previous_frames = previous.frames,
        previous_monotonic_ns = previous.monotonic_ns,
        current_clock_id = current.clock_id,
        current_rate = current.rate,
        current_position = current.position,
        current_frames = current.frames,
        current_monotonic_ns = current.monotonic_ns,
        mapped_epoch = read.event.mapped_epoch,
        render_position = read.event.render_position,
        "PipeWire native timeline discontinuity"
    );
}

#[cfg(test)]
#[path = "linux_driver_monitor/tests.rs"]
mod tests;
