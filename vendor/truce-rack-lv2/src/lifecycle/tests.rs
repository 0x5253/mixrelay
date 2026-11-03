use super::*;
use crate::atoms::{MAX_LV2_EVENT_BUFFER_BYTES, TIME_POSITION_MAX_BYTES};
use lv2_raw::atom::{LV2AtomEvent, LV2AtomSequence};

#[derive(Default)]
struct FakeActivation {
    calls: Vec<&'static str>,
    restore_failure: bool,
    activation_failure: bool,
}

impl ActivationActions for FakeActivation {
    fn prepare_instance(&mut self) -> Result<()> {
        self.calls.push("prepare");
        Ok(())
    }

    fn restore(&mut self, state: &[u8]) -> Result<()> {
        assert_eq!(state, b"fixture-state");
        self.calls.push("restore");
        if self.restore_failure {
            return Err(Error::Other("fake restore failure".into()));
        }
        Ok(())
    }

    fn activate(&mut self) -> Result<()> {
        self.calls.push("activate");
        if self.activation_failure {
            return Err(Error::Other("fake activation failure".into()));
        }
        Ok(())
    }

    fn discard_after_restore_failure(&mut self) {
        self.calls.push("discard");
    }
}

#[test]
fn state_is_restored_before_native_activation() {
    let mut lifecycle = FakeActivation::default();
    restore_then_activate(&mut lifecycle, Some(b"fixture-state")).unwrap();
    assert_eq!(lifecycle.calls, ["prepare", "restore", "activate"]);
}

#[test]
fn no_state_uses_the_shared_activation_path_without_restore() {
    let mut lifecycle = FakeActivation::default();
    restore_then_activate(&mut lifecycle, None).unwrap();
    assert_eq!(lifecycle.calls, ["prepare", "activate"]);
}

#[test]
fn failed_restore_discards_instance_and_never_activates() {
    let mut lifecycle = FakeActivation {
        restore_failure: true,
        ..FakeActivation::default()
    };
    let error = restore_then_activate(&mut lifecycle, Some(b"fixture-state")).unwrap_err();
    assert_eq!(error.to_string(), "fake restore failure");
    assert_eq!(lifecycle.calls, ["prepare", "restore", "discard"]);
}

#[test]
fn activation_failure_is_propagated_after_restore() {
    let mut lifecycle = FakeActivation {
        activation_failure: true,
        ..FakeActivation::default()
    };
    let error = restore_then_activate(&mut lifecycle, Some(b"fixture-state")).unwrap_err();
    assert_eq!(error.to_string(), "fake activation failure");
    assert_eq!(lifecycle.calls, ["prepare", "restore", "activate"]);
}

#[test]
fn same_rate_keeps_the_deactivated_native_instance() {
    assert!(!configuration_requires_new_instance(
        true, 48_000.0, 48_000.0, 128, 128
    ));
    assert!(configuration_requires_new_instance(
        true, 48_000.0, 44_100.0, 128, 128
    ));
    assert!(configuration_requires_new_instance(
        false, 48_000.0, 48_000.0, 128, 128
    ));
    assert!(configuration_requires_new_instance(
        true, 48_000.0, 48_000.0, 128, 256
    ));
}

#[test]
fn activation_validation_rejects_unsupported_layouts_and_invalid_rates() {
    let supported = [BusLayout::stereo()];
    assert!(
        validate_activation_request(false, &supported, &BusLayout::mono(), 48_000.0, 128,).is_err()
    );
    for sample_rate in [0.0, f64::NAN, f64::INFINITY] {
        assert!(
            validate_activation_request(false, &supported, &supported[0], sample_rate, 128,)
                .is_err()
        );
    }
    assert!(validate_activation_request(true, &supported, &supported[0], 48_000.0, 128,).is_err());
}

#[test]
fn activation_validation_rejects_unbounded_or_zero_block_sizes() {
    let supported = [BusLayout::stereo()];
    for max_block_size in [0, usize::MAX] {
        assert!(
            validate_activation_request(
                false,
                &supported,
                &supported[0],
                48_000.0,
                max_block_size,
            )
            .is_err()
        );
    }
    assert!(midi_buffer_capacity(4_096).is_ok());
    let overhead = std::mem::size_of::<LV2AtomSequence>() + TIME_POSITION_MAX_BYTES;
    let event_slot = std::mem::size_of::<LV2AtomEvent>() + 8;
    let maximum_frames = (MAX_LV2_EVENT_BUFFER_BYTES - overhead) / event_slot;
    assert!(midi_buffer_capacity(maximum_frames).is_ok());
    assert!(midi_buffer_capacity(maximum_frames + 1).is_err());
}
