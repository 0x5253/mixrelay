//! Explicit two-output startup on a real private aggregate. Sends silence only.
use super::*;
use crate::macos_devices::{device::Device, host::CoreAudio, spec::Spec};
use mixrelay_audio_io::DeviceSelector;

#[test]
fn native_outputs_start_without_aggregate() {
    let host: Arc<dyn Host> = Arc::new(CoreAudio::default());
    for uid in std::env::var("MIXRELAY_LAB_AGGREGATE_OUTPUT_UIDS")
        .unwrap()
        .lines()
    {
        let spec = Spec {
            selector: DeviceSelector::PersistentUid { uid: uid.into() },
            input: false,
            channels: 2,
        };
        let device = Device::open_with_group(
            &spec,
            48_000,
            None,
            crate::macos_devices::device::CallbackConfig {
                buffer_frames: 128,
                output_only: true,
            },
            host.clone(),
        )
        .unwrap();
        assert!(device.ready());
        drop(device);
        crate::macos_devices::worker_retirement::WORKERS
            .finish()
            .unwrap();
    }
}

#[test]
fn native_aggregate_starts_each_member_and_retires_every_registration() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let members: Vec<_> = std::env::var("MIXRELAY_LAB_AGGREGATE_OUTPUT_UIDS")
        .expect("two explicitly selected output UIDs, separated by a newline")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(members.len(), 2);
    let host: Arc<dyn Host> = Arc::new(CoreAudio::default());
    let group = Aggregate::create(members.clone(), &host)
        .unwrap()
        .expect("the two selected devices must support a private group");
    let mut devices = Vec::new();
    for uid in members {
        println!("starting aggregate output {uid}");
        let spec = Spec {
            selector: DeviceSelector::PersistentUid { uid },
            input: false,
            channels: 2,
        };
        let device = Device::open_with_group(
            &spec,
            48_000,
            Some(group.clone()),
            crate::macos_devices::device::CallbackConfig {
                buffer_frames: 128,
                output_only: true,
            },
            host.clone(),
        )
        .unwrap();
        assert_ne!(host.ticks(&device.native), 0, "member never delivered PCM");
        devices.push(device);
    }
    assert!(devices.iter().all(Device::ready));
    for device in devices {
        device
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        // Drop's normal tracked retirement owns and joins each native worker.
        drop(device);
    }
    crate::macos_devices::worker_retirement::WORKERS
        .finish()
        .unwrap();
    drop(group);
    Aggregate::reclaim_retired();
}
