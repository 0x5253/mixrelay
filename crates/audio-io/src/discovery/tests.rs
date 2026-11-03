use std::collections::BTreeMap;

use crate::{DeviceDirection, DeviceSelector};

use super::{
    AudioDeviceDescriptor, DeviceInventory, DeviceObservation, HostAudioBackendError, inventory,
};

#[derive(Default)]
struct FakeReader {
    devices: BTreeMap<u32, DeviceObservation>,
    input: Option<u32>,
    output: Option<u32>,
    failed: bool,
    reads: std::cell::RefCell<Vec<u32>>,
}

impl DeviceInventory for FakeReader {
    type Id = u32;
    fn resolve_uid(&self, uid: &str) -> Result<Option<u32>, HostAudioBackendError> {
        Ok(self
            .devices
            .iter()
            .find_map(|(id, info)| (info.descriptor.uid == uid).then_some(*id)))
    }
    fn device_ids(&self) -> Result<Vec<u32>, HostAudioBackendError> {
        if self.failed {
            return Err(HostAudioBackendError::BackendFailure {
                message: "injected discovery failure".into(),
            });
        }
        // 999 models a device that vanished after the system ID list was read.
        Ok(self.devices.keys().copied().chain([999]).collect())
    }
    fn default_input(&self) -> Option<u32> {
        self.input
    }
    fn default_output(&self) -> Option<u32> {
        self.output
    }
    fn device(&self, id: u32) -> Option<DeviceObservation> {
        self.reads.borrow_mut().push(id);
        self.devices.get(&id).cloned()
    }
}

fn info(uid: &str, input: u32, output: u32) -> DeviceObservation {
    DeviceObservation {
        descriptor: AudioDeviceDescriptor {
            uid: uid.into(),
            display_name: format!("Name {uid}"),
            input_channels: input,
            output_channels: output,
            is_default_input: false,
            is_default_output: false,
        },
        alive: true,
        hidden: false,
    }
}

#[test]
fn default_and_explicit_uid_resolve_to_same_physical_capture_owner() {
    let reader = FakeReader {
        devices: [
            (7, info("physical-interface", 7, 2)),
            (8, info("output-only", 0, 8)),
        ]
        .into(),
        input: Some(7),
        output: Some(7),
        failed: false,
        reads: std::cell::RefCell::default(),
    };
    let devices = inventory(&reader).unwrap();
    let explicit = DeviceSelector::PersistentUid {
        uid: "physical-interface".into(),
    };
    let by_default = DeviceSelector::DefaultInput
        .resolve(&devices, DeviceDirection::Input, 7)
        .unwrap();
    let by_uid = explicit
        .resolve(&devices, DeviceDirection::Input, 7)
        .unwrap();
    assert!(std::ptr::eq(by_default, by_uid));
    assert_eq!(by_uid.output_channels, 2);
    assert!(
        explicit
            .resolve(&devices, DeviceDirection::Input, 8)
            .is_err()
    );
    assert!(
        DeviceSelector::DefaultOutput
            .resolve(&devices, DeviceDirection::Output, 2)
            .is_ok()
    );
}

#[test]
fn selected_recovery_never_queries_unrelated_geometry_or_system_inventory() {
    let reader = FakeReader {
        devices: (1..=100)
            .map(|id| (id, info(&format!("device-{id}"), 2, 2)))
            .collect(),
        input: Some(7),
        output: Some(7),
        ..FakeReader::default()
    };
    let selectors = [
        DeviceSelector::PersistentUid {
            uid: "device-7".into(),
        },
        DeviceSelector::DefaultInput,
        DeviceSelector::DefaultOutput,
        DeviceSelector::PersistentUid {
            uid: "device-13".into(),
        },
    ];
    inventory(&reader).unwrap();
    assert_eq!(reader.reads.borrow().len(), 101);
    reader.reads.borrow_mut().clear();
    let reader = FakeReader {
        failed: true,
        ..reader
    };
    let selected = super::selected_inventory(&reader, &selectors).unwrap();
    assert_eq!(*reader.reads.borrow(), [7, 13]);
    assert_eq!(selected.len(), 2);
    assert!(selected[0].is_default_input && selected[0].is_default_output);
}

#[test]
fn selected_absence_and_private_devices_preserve_explicit_intent() {
    let mut reader = FakeReader {
        input: Some(7),
        ..FakeReader::default()
    };
    let mut private = info("private", 2, 2);
    private.hidden = true;
    reader.devices.insert(7, private);
    assert!(
        super::selected_inventory(&reader, &[DeviceSelector::DefaultInput])
            .unwrap()
            .is_empty()
    );
    let explicit = DeviceSelector::PersistentUid {
        uid: "private".into(),
    };
    let selected = super::selected_inventory(&reader, &[explicit]).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].uid, "private");
    assert!(
        super::selected_inventory(
            &reader,
            &[DeviceSelector::PersistentUid {
                uid: "absent".into()
            }]
        )
        .unwrap()
        .is_empty()
    );
    reader.reads.borrow_mut().clear();
    assert!(super::selected_inventory(&reader, &[]).unwrap().is_empty());
    assert!(reader.reads.borrow().is_empty());
}

#[test]
fn absent_and_filtered_defaults_do_not_fall_back_to_other_devices() {
    let mut reader = FakeReader::default();
    reader.devices.insert(1, info("live", 1, 2));
    let mut dead = info("dead", 2, 2);
    dead.alive = false;
    reader.devices.insert(2, dead);
    let mut hidden = info("private-aggregate", 2, 2);
    hidden.hidden = true;
    reader.devices.insert(3, hidden);
    reader.devices.insert(4, info("", 2, 2));
    reader.devices.insert(5, info("no-streams", 0, 0));
    let mut unnamed = info("unnamed", 2, 2);
    unnamed.descriptor.display_name.clear();
    reader.devices.insert(6, unnamed);
    reader.input = Some(2);
    reader.output = Some(999);
    let devices = inventory(&reader).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].uid, "live");
    assert!(
        DeviceSelector::DefaultInput
            .resolve(&devices, DeviceDirection::Input, 1)
            .is_err()
    );
    assert!(
        DeviceSelector::DefaultOutput
            .resolve(&devices, DeviceDirection::Output, 1)
            .is_err()
    );
    reader.failed = true;
    assert!(inventory(&reader).is_err());
}
