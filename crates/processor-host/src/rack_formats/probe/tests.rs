//! Catalog negotiation against a scripted native plug-in.

use std::sync::atomic::Ordering;

use truce_rack_core::{PluginCore, bus::BusLayout};

use super::{describe_loaded_plugin, normalize_parameter};
use crate::rack_formats::{
    fixture::{FixtureActivationFailure, fixture},
    identity::{RackFormat, RackLocator},
    layouts,
};

#[test]
fn catalog_omits_rejected_layout_and_chooses_only_a_verified_default() {
    let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
    plugin.activation_failure = Some((2, FixtureActivationFailure::Reusable));
    let locator = RackLocator::from_info(RackFormat::Au, plugin.info.clone());
    let record = describe_loaded_plugin(locator, &mut plugin).unwrap();
    assert_eq!(record.descriptor.layouts.len(), 1);
    assert_eq!(record.descriptor.layouts[0].total_input_channels(), Ok(1));
    assert_eq!(
        record.descriptor.default_layout_id,
        record.descriptor.layouts[0].layout_id
    );
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 3);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 2);
    assert!(!plugin.is_active());
}

#[test]
fn probe_descriptor_takes_editor_capability_from_the_loaded_instance() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    let mut scan_hint = plugin.info.clone();
    scan_hint.has_editor = true;
    let locator = RackLocator::from_info(RackFormat::Clap, scan_hint);

    let record = describe_loaded_plugin(locator, &mut plugin).unwrap();

    assert!(!record.descriptor.has_editor);
    assert!(!record.locator.has_editor);
    assert_eq!(record.descriptor.type_id, "clap:channel-fixture");
    assert_eq!(record.descriptor.parameters.len(), 1);
    assert_eq!(record.descriptor.parameters[0].id, plugin.parameter_id);
    assert!((record.descriptor.parameters[0].default_value - 0.5).abs() < f64::EPSILON);
}

#[test]
fn catalog_preserves_cause_when_every_layout_is_rejected() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    plugin.activation_failure = Some((2, FixtureActivationFailure::Reusable));
    let error = layouts::verified_layouts(&mut plugin).unwrap_err();
    assert!(error.contains("no usable audio effect layout"), "{error}");
    assert!(error.contains("fixture rejects 2 channels"), "{error}");
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 0);
}

#[test]
fn catalog_does_not_hide_rollback_or_active_rejection_failures() {
    for failure in [
        FixtureActivationFailure::Fatal,
        FixtureActivationFailure::StillActive,
    ] {
        let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
        plugin.activation_failure = Some((2, failure));
        assert!(layouts::verified_layouts(&mut plugin).is_err());
        assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
        assert!(!plugin.is_active());
    }
}

#[test]
fn catalog_requires_successful_deactivation_before_the_next_configuration() {
    let mut plugin = fixture(vec![BusLayout::stereo(), BusLayout::mono()]);
    plugin.refuse_deactivation_after = Some(1);
    let error = layouts::verified_layouts(&mut plugin).unwrap_err();
    assert!(error.contains("did not deactivate"), "{error}");
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
}

#[test]
fn catalog_does_not_publish_after_final_default_teardown_fails() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    plugin.refuse_deactivation_after = Some(2);
    let locator = RackLocator::from_info(RackFormat::Au, plugin.info.clone());
    let error = describe_loaded_plugin(locator, &mut plugin).unwrap_err();
    assert!(
        error.contains("default layout did not deactivate"),
        "{error}"
    );
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 2);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 2);
    assert!(plugin.is_active());
}

#[test]
fn large_catalog_still_deduplicates_and_rejects_over_limit_channels() {
    let mut plugin = fixture(vec![BusLayout::stereo(); 80]);
    assert_eq!(layouts::verified_layouts(&mut plugin).unwrap().len(), 1);
    assert_eq!(plugin.activations.load(Ordering::Relaxed), 1);
    assert_eq!(plugin.deactivations.load(Ordering::Relaxed), 1);
    let invalid = plugin.layouts.last_mut().unwrap();
    invalid.outputs[0].channels = truce_rack_core::bus::ChannelConfig::Discrete(65);
    assert!(layouts::verified_layouts(&mut plugin).is_err());
    assert!(!plugin.is_active());
}

#[test]
fn parameter_normalization_is_bounded_and_handles_invalid_ranges() {
    assert!((normalize_parameter(0.0, -1.0, 1.0) - 0.5).abs() < f64::EPSILON);
    assert!(normalize_parameter(-4.0, -1.0, 1.0).abs() < f64::EPSILON);
    assert!((normalize_parameter(4.0, -1.0, 1.0) - 1.0).abs() < f64::EPSILON);
    assert!(normalize_parameter(1.0, 1.0, 1.0).abs() < f64::EPSILON);
    assert!(normalize_parameter(f64::NAN, 0.0, 1.0).abs() < f64::EPSILON);
}
