//! Selection is a product contract; native IDs and stream creation belong to adapters.
use crate::{AudioDeviceDescriptor, HostAudioBackendError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum DeviceSelector {
    PersistentUid { uid: String },
    DefaultInput,
    DefaultOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceDirection {
    Input,
    Output,
}

impl DeviceSelector {
    /// Resolves exactly one device in a current inventory, never falling back
    /// from a missing explicit UID to another device or inventing channel width.
    /// # Errors
    /// Rejects wrong directions, missing/ambiguous devices, and unsupported widths.
    pub fn resolve<'a>(
        &self,
        devices: &'a [AudioDeviceDescriptor],
        direction: DeviceDirection,
        channels: u16,
    ) -> Result<&'a AudioDeviceDescriptor, HostAudioBackendError> {
        let invalid = || HostAudioBackendError::CapabilityUnavailable {
            capability: "requested audio device, direction or channel width is unavailable".into(),
        };
        if channels == 0
            || channels > 64
            || matches!(
                (self, direction),
                (Self::DefaultInput, DeviceDirection::Output)
                    | (Self::DefaultOutput, DeviceDirection::Input)
            )
        {
            return Err(invalid());
        }
        let mut matches = devices.iter().filter(|device| match self {
            Self::PersistentUid { uid } => !uid.is_empty() && &device.uid == uid,
            Self::DefaultInput => device.is_default_input,
            Self::DefaultOutput => device.is_default_output,
        });
        let device = matches.next().ok_or_else(invalid)?;
        if matches.next().is_some() {
            return Err(invalid());
        }
        let available = match direction {
            DeviceDirection::Input => device.input_channels,
            DeviceDirection::Output => device.output_channels,
        };
        if available < u32::from(channels) {
            return Err(invalid());
        }
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selectors_keep_direction_identity_and_channel_contract_on_every_os() {
        let devices = vec![AudioDeviceDescriptor {
            uid: "interface-64".into(),
            display_name: "Interface".into(),
            input_channels: 64,
            output_channels: 16,
            is_default_input: true,
            is_default_output: true,
        }];
        for channels in [1, 2, 8, 16, 32, 64] {
            assert!(
                DeviceSelector::DefaultInput
                    .resolve(&devices, DeviceDirection::Input, channels)
                    .is_ok()
            );
            assert_eq!(
                DeviceSelector::DefaultOutput
                    .resolve(&devices, DeviceDirection::Output, channels)
                    .is_ok(),
                channels <= 16
            );
        }
        assert!(
            DeviceSelector::DefaultInput
                .resolve(&devices, DeviceDirection::Output, 2)
                .is_err()
        );
        assert!(
            DeviceSelector::DefaultOutput
                .resolve(&devices, DeviceDirection::Input, 2)
                .is_err()
        );
        assert!(
            DeviceSelector::PersistentUid {
                uid: "missing".into()
            }
            .resolve(&devices, DeviceDirection::Input, 2)
            .is_err()
        );
        assert!(
            DeviceSelector::DefaultInput
                .resolve(
                    &[devices[0].clone(), devices[0].clone()],
                    DeviceDirection::Input,
                    2
                )
                .is_err()
        );
        let mut unknown = devices;
        unknown[0].input_channels = 0;
        assert!(
            DeviceSelector::DefaultInput
                .resolve(&unknown, DeviceDirection::Input, 1)
                .is_err()
        );
    }
    #[test]
    fn moving_selector_into_io_keeps_existing_graph_wire_format() {
        let selector = DeviceSelector::PersistentUid {
            uid: "stable-device".into(),
        };
        assert_eq!(
            serde_json::to_value(selector).unwrap(),
            serde_json::json!({"match":"persistent_uid","uid":"stable-device"})
        );
        assert_eq!(
            serde_json::from_str::<DeviceSelector>(r#"{"match":"default_input"}"#).unwrap(),
            DeviceSelector::DefaultInput
        );
    }
}
