use std::collections::VecDeque;

use mixrelay_audio_io::{
    VirtualEndpointCapabilities, VirtualEndpointDeviceSettings, VirtualEndpointId,
    VirtualEndpointSettings, VirtualEndpointSpec,
};
use parking_lot::Mutex;

use super::*;

/// Answers snapshots and mutations from scripts and records every command it was sent.
#[derive(Default)]
struct ScriptedDriver {
    snapshots: Mutex<VecDeque<Result<DriverSnapshot, DriverError>>>,
    replies: Mutex<VecDeque<Result<DriverSnapshot, DriverError>>>,
    sent: Mutex<Vec<DriverCommand>>,
}

impl DriverRequests for ScriptedDriver {
    fn snapshot(&self) -> impl Future<Output = Result<DriverSnapshot, DriverError>> + Send {
        let snapshot = self
            .snapshots
            .lock()
            .pop_front()
            .expect("a scripted snapshot");
        async move { snapshot }
    }

    fn mutate(
        &self,
        command: &DriverCommand,
    ) -> impl Future<Output = Result<DriverSnapshot, DriverError>> + Send {
        self.sent.lock().push(command.clone());
        let reply = self.replies.lock().pop_front().expect("a scripted reply");
        async move { reply }
    }
}

fn snapshot(revision: u64, state: DriverState, devices: bool) -> DriverSnapshot {
    let mut settings = VirtualEndpointSettings::configurable(&VirtualEndpointCapabilities {
        max_render_channels: 2,
        max_capture_channels: 2,
        supported_sample_rates: vec![48_000],
        supports_duplex: true,
        supports_dynamic_creation: true,
    });
    if devices {
        settings.devices.push(VirtualEndpointDeviceSettings {
            spec: VirtualEndpointSpec {
                id: VirtualEndpointId::new("relay-1".into()),
                display_name: "MixRelay 1".into(),
                render: None,
                capture: None,
                supported_sample_rates: vec![48_000],
                default_sample_rate: 48_000,
            },
            native_device_ids: Vec::new(),
            observed_sample_rate: None,
            supported_sample_rates: vec![48_000],
            can_set_sample_rate: false,
            native_formats: mixrelay_audio_io::NativeDeviceFormats::default(),
        });
    }
    DriverSnapshot {
        protocol_version: 1,
        revision,
        state,
        installed_version: None,
        owner_process_id: None,
        settings,
        persistence_error: None,
    }
}

fn conflict() -> Result<DriverSnapshot, DriverError> {
    Err(DriverError::Remote {
        status: 409,
        message: "revision".into(),
    })
}

fn driver(
    snapshots: impl IntoIterator<Item = Result<DriverSnapshot, DriverError>>,
    replies: impl IntoIterator<Item = Result<DriverSnapshot, DriverError>>,
) -> ScriptedDriver {
    ScriptedDriver {
        snapshots: Mutex::new(snapshots.into_iter().collect()),
        replies: Mutex::new(replies.into_iter().collect()),
        sent: Mutex::default(),
    }
}

#[tokio::test]
async fn each_attempt_reads_a_fresh_revision_after_a_conflict() {
    let ready = |revision| snapshot(revision, DriverState::Ready, false);
    let driver = driver(
        [Ok(ready(1)), Ok(ready(2)), Ok(ready(3))],
        [conflict(), conflict(), Ok(ready(4))],
    );
    assert_eq!(set_active(&driver, true).await.unwrap().revision, 4);
    assert_eq!(
        *driver.sent.lock(),
        [1, 2, 3].map(|expected_revision| DriverCommand::Activate { expected_revision })
    );
}

#[tokio::test]
async fn the_third_conflict_is_returned() {
    let ready = |revision| Ok(snapshot(revision, DriverState::Ready, false));
    let driver = driver(
        [ready(1), ready(2), ready(3)],
        [conflict(), conflict(), conflict()],
    );
    assert!(matches!(
        set_active(&driver, false).await,
        Err(DriverError::Remote { status: 409, .. })
    ));
    assert_eq!(
        *driver.sent.lock(),
        [1, 2, 3].map(|expected_revision| DriverCommand::Deactivate { expected_revision })
    );
}

fn unreachable_service() -> Result<DriverSnapshot, DriverError> {
    Err(DriverError::NotReachable("no socket".into()))
}

#[tokio::test]
async fn a_failed_snapshot_is_returned_without_sending_anything() {
    let driver = driver([unreachable_service()], []);
    assert!(matches!(
        set_active(&driver, false).await,
        Err(DriverError::NotReachable(_))
    ));
    assert!(driver.sent.lock().is_empty());
}

#[tokio::test]
async fn a_failed_snapshot_after_a_conflict_is_returned() {
    let driver = driver(
        [
            Ok(snapshot(1, DriverState::Ready, false)),
            unreachable_service(),
        ],
        [conflict()],
    );
    assert!(matches!(
        set_active(&driver, true).await,
        Err(DriverError::NotReachable(_))
    ));
    assert_eq!(driver.sent.lock().len(), 1);
}

#[tokio::test]
async fn other_failures_are_not_retried() {
    let driver = driver(
        [Ok(snapshot(1, DriverState::Ready, false))],
        [Err(DriverError::Unavailable("native owner".into()))],
    );
    assert!(matches!(
        set_active(&driver, true).await,
        Err(DriverError::Unavailable(_))
    ));
    assert_eq!(driver.sent.lock().len(), 1);
}

#[tokio::test]
async fn stopping_a_driver_with_nothing_installed_sends_nothing() {
    let absent = snapshot(7, DriverState::NotInstalled, false);
    let driver = driver([Ok(absent.clone())], []);
    assert_eq!(set_active(&driver, false).await.unwrap(), absent);
    assert!(driver.sent.lock().is_empty());
}

#[tokio::test]
async fn devices_or_activation_still_reach_a_driver_that_is_not_installed() {
    for (active, devices) in [(false, true), (true, false)] {
        let state = snapshot(1, DriverState::NotInstalled, devices);
        let driver = driver([Ok(state.clone())], [Ok(state)]);
        set_active(&driver, active).await.unwrap();
        assert_eq!(
            driver.sent.lock().len(),
            1,
            "active {active}, devices {devices}"
        );
    }
}
