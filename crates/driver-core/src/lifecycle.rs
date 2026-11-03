//! Starting and stopping native devices against the service's settings revision.
use crate::{DriverCommand, DriverError, DriverSnapshot, DriverState};

/// Another settings client may change the revision between reading and sending. Three
/// attempts, each with a fresh revision, ride out a burst of edits from the other
/// windows without letting a persistent conflict stall the caller.
const REVISION_ATTEMPTS: usize = 3;

/// The two Driver service requests activation needs.
pub trait DriverRequests {
    fn snapshot(&self) -> impl Future<Output = Result<DriverSnapshot, DriverError>> + Send;
    fn mutate(
        &self,
        command: &DriverCommand,
    ) -> impl Future<Output = Result<DriverSnapshot, DriverError>> + Send;
}

/// Activates or deactivates at the current revision, retrying only a revision conflict.
/// Stopping a Driver that is not installed and has no devices succeeds without a request.
/// # Errors
/// Returns snapshot, mutation or exhausted revision-conflict errors.
pub async fn set_active(
    driver: &impl DriverRequests,
    active: bool,
) -> Result<DriverSnapshot, DriverError> {
    let mut attempt = 1;
    loop {
        let snapshot = driver.snapshot().await?;
        if !active
            && snapshot.state == DriverState::NotInstalled
            && snapshot.settings.devices.is_empty()
        {
            return Ok(snapshot);
        }
        let expected_revision = snapshot.revision;
        let command = if active {
            DriverCommand::Activate { expected_revision }
        } else {
            DriverCommand::Deactivate { expected_revision }
        };
        match driver.mutate(&command).await {
            Err(DriverError::Remote { status: 409, .. }) if attempt < REVISION_ATTEMPTS => {
                attempt += 1;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests;
