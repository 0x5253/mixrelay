#[cfg(feature = "native-lab")]
use mixrelay_audio_io::DeviceDirection;
use mixrelay_audio_io::DeviceSelector;

#[test]
fn selected_native_uid_rejects_invalid_strings_before_querying() {
    for uid in ["", "invalid\0uid"] {
        assert!(
            super::selected_devices(&[DeviceSelector::PersistentUid { uid: uid.into() }]).is_err()
        );
    }
}

#[cfg(feature = "native-lab")]
#[test]
fn actual_coreaudio_inventory_has_resolvable_device_identity_without_mutation() {
    let devices = super::devices().expect("actual CoreAudio inventory must be readable");
    assert!(
        !devices.is_empty(),
        "this macOS host must expose at least one audio device"
    );
    let mut uids = std::collections::BTreeSet::new();
    for device in &devices {
        assert!(uids.insert(&device.uid), "device UIDs must be unique");
        assert!(!device.uid.is_empty() && !device.display_name.is_empty());
        assert!(device.input_channels > 0 || device.output_channels > 0);
        let direction = if device.input_channels > 0 {
            DeviceDirection::Input
        } else {
            DeviceDirection::Output
        };
        let selected = DeviceSelector::PersistentUid {
            uid: device.uid.clone(),
        };
        assert!(std::ptr::eq(
            selected.resolve(&devices, direction, 1).unwrap(),
            device
        ));
    }
    assert!(
        devices
            .iter()
            .filter(|device| device.is_default_input)
            .count()
            <= 1
    );
    assert!(
        devices
            .iter()
            .filter(|device| device.is_default_output)
            .count()
            <= 1
    );
}
