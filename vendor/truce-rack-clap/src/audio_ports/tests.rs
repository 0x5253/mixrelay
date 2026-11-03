//! Audio port catalog and layout negotiation regressions.

use super::{
    Bus, BusKind, BusLayout, ChannelConfig, PortDescriptions, bus_from_info, layout_identity_id,
    parse_layout_identity, restoration_config_id, same_topology, validate_cached_live_layout,
    validate_same_native_ports,
};
use clap_sys::ext::audio_ports::{CLAP_AUDIO_PORT_IS_MAIN, CLAP_PORT_STEREO, clap_audio_port_info};
use clap_sys::string_sizes::CLAP_NAME_SIZE;
use std::ffi::c_char;
use std::ptr;

fn port_info(name: &[u8], flags: u32, channels: u32) -> clap_audio_port_info {
    let mut name_buffer = [0; CLAP_NAME_SIZE];
    for (destination, source) in name_buffer.iter_mut().zip(name.iter().copied()) {
        *destination = c_char::from_ne_bytes([source]);
    }
    clap_audio_port_info {
        id: 7,
        name: name_buffer,
        flags,
        channel_count: channels,
        port_type: if channels == 2 {
            CLAP_PORT_STEREO.as_ptr()
        } else {
            ptr::null()
        },
        in_place_pair: 0,
    }
}

fn topology(name: &str, kind: BusKind, channels: ChannelConfig) -> BusLayout {
    let mut layout = BusLayout::new();
    layout.inputs.push(Bus {
        name: name.to_owned(),
        kind,
        channels,
    });
    layout.outputs.push(Bus {
        name: "Output".to_owned(),
        kind: BusKind::Main,
        channels: ChannelConfig::Stereo,
    });
    layout
}

fn identified_layout(
    layout: &BusLayout,
    config_id: Option<u32>,
    input_ids: &[u32],
    output_ids: &[u32],
) -> BusLayout {
    let mut layout = layout.clone();
    layout.id =
        Some(layout_identity_id(config_id, input_ids, output_ids).expect("native layout identity"));
    layout
}

fn main_and_sidechain_topology() -> BusLayout {
    let mut layout = BusLayout::new();
    layout.inputs.extend([
        Bus {
            name: "Main input".into(),
            kind: BusKind::Main,
            channels: ChannelConfig::Stereo,
        },
        Bus {
            name: "Sidechain".into(),
            kind: BusKind::Sidechain,
            channels: ChannelConfig::Stereo,
        },
    ]);
    layout.outputs.push(Bus {
        name: "Main output".into(),
        kind: BusKind::Main,
        channels: ChannelConfig::Stereo,
    });
    layout
}

#[test]
fn maps_native_port_role_and_known_or_discrete_channel_shapes() {
    let main = port_info(b"Main", CLAP_AUDIO_PORT_IS_MAIN, 2);
    let sidechain = port_info(b"Key", 0, 2);
    let unknown = port_info(b"Discrete", 0, 6);

    let main = bus_from_info(&main, true, 0).expect("valid main port");
    let sidechain = bus_from_info(&sidechain, true, 1).expect("valid sidechain port");
    let discrete = bus_from_info(&unknown, true, 2).expect("valid unknown port");
    let mut untyped_stereo = port_info(b"Unspecified 2ch", 0, 2);
    untyped_stereo.port_type = ptr::null();
    let discrete_two = bus_from_info(&untyped_stereo, true, 3).expect("valid untyped port");

    assert_eq!(main.kind, BusKind::Main);
    assert_eq!(main.channels, ChannelConfig::Stereo);
    assert_eq!(sidechain.kind, BusKind::Sidechain);
    assert_eq!(discrete.channels, ChannelConfig::Discrete(6));
    assert_eq!(discrete_two.channels, ChannelConfig::Discrete(2));
}

#[test]
fn duplicate_native_topologies_keep_distinct_configuration_ids() {
    let shape = topology("Localized main", BusKind::Main, ChannelConfig::Stereo);
    let first = identified_layout(&shape, Some(1), &[10], &[20]);
    let second = identified_layout(&shape, Some(2), &[10], &[20]);
    let catalog = super::AudioPortCatalog::test_catalog(vec![first, second]);

    assert_eq!(catalog.layouts().len(), 2);
    assert_ne!(catalog.layouts()[0].id, catalog.layouts()[1].id);
    assert_eq!(
        parse_layout_identity(catalog.layouts()[0].id.as_deref().expect("identity"))
            .expect("config identity")
            .config_id,
        Some(1)
    );
    assert_eq!(
        parse_layout_identity(catalog.layouts()[1].id.as_deref().expect("identity"))
            .expect("config identity")
            .config_id,
        Some(2)
    );
}

#[test]
fn rejects_a_main_bus_after_the_first_native_bus() {
    let sidechain = Bus {
        name: "Key".into(),
        kind: BusKind::Sidechain,
        channels: ChannelConfig::Stereo,
    };
    let main = Bus {
        name: "Main".into(),
        kind: BusKind::Main,
        channels: ChannelConfig::Stereo,
    };
    assert!(
        super::validate_main_bus(
            true,
            2,
            CLAP_PORT_STEREO.as_ptr(),
            &[sidechain, main],
            "input"
        )
        .is_err()
    );
}

#[test]
fn topology_validation_ignores_localized_names_but_keeps_bus_order_and_shape() {
    let original = topology("Main", BusKind::Main, ChannelConfig::Stereo);
    let localized = topology("Haupt", BusKind::Main, ChannelConfig::Stereo);
    let changed = topology("Main", BusKind::Sidechain, ChannelConfig::Stereo);

    assert!(same_topology(&original, &localized));
    assert!(!same_topology(&original, &changed));
}

#[test]
fn parses_native_clap_configuration_ids_without_numeric_truncation() {
    let largest_id =
        layout_identity_id(Some(u32::MAX - 1), &[7], &[8]).expect("valid largest config ID");
    assert_eq!(
        parse_layout_identity(&largest_id)
            .expect("valid parsed layout identity")
            .config_id,
        Some(u32::MAX - 1)
    );
    assert!(
        parse_layout_identity(
            "clap:4294967295:0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err()
    );
    assert!(
        parse_layout_identity(
            "clap:4294967296:0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err()
    );
    assert!(parse_layout_identity("vst3:42").is_err());
}

#[test]
fn same_shape_native_port_reordering_is_rejected() {
    let shape = main_and_sidechain_topology();
    let selected = identified_layout(&shape, Some(2), &[10, 11], &[20]);
    let reordered = identified_layout(&shape, Some(2), &[11, 10], &[20]);

    assert!(validate_cached_live_layout(&selected, &reordered).is_err());
}

#[test]
fn stable_native_port_ids_allow_display_name_changes() {
    let shape = main_and_sidechain_topology();
    let selected = identified_layout(&shape, Some(2), &[10, 11], &[20]);
    let mut localized_shape = shape;
    localized_shape.inputs[0].name = "Localized main".into();
    localized_shape.inputs[1].name = "Localized key".into();
    localized_shape.outputs[0].name = "Localized output".into();
    let renamed = identified_layout(&localized_shape, Some(2), &[10, 11], &[20]);

    assert_eq!(selected.id, renamed.id);
    assert!(same_topology(&selected, &renamed));
    assert!(validate_cached_live_layout(&selected, &renamed).is_ok());
}

#[test]
fn initial_topology_without_config_info_restores_by_ordered_native_ids() {
    let shape = main_and_sidechain_topology();
    let initial = identified_layout(&shape, None, &[10, 11], &[20]);
    let different_ids = identified_layout(&shape, Some(1), &[10, 12], &[20]);
    let original_ids = identified_layout(&shape, Some(2), &[10, 11], &[20]);

    let restored = restoration_config_id(&initial, &[different_ids, original_ids])
        .expect("find config matching initial native IDs");
    assert_eq!(restored, 2);
}

#[test]
fn restored_current_identity_may_differ_only_by_config_id() {
    let shape = main_and_sidechain_topology();
    let initial = identified_layout(&shape, None, &[10, 11], &[20]);
    let restored = identified_layout(&shape, Some(2), &[10, 11], &[20]);

    assert!(validate_same_native_ports(&initial, &restored).is_ok());
}

#[test]
fn native_port_ids_must_be_unique_per_direction_not_across_directions() {
    let shape = topology("Main", BusKind::Main, ChannelConfig::Stereo);
    let inputs = PortDescriptions {
        buses: shape.inputs.to_vec(),
        native_ids: vec![7],
    };
    let outputs = PortDescriptions {
        buses: shape.outputs.to_vec(),
        native_ids: vec![7],
    };
    assert!(super::build_layout(None, inputs, outputs).is_ok());

    let duplicate_inputs = PortDescriptions {
        buses: main_and_sidechain_topology().inputs.to_vec(),
        native_ids: vec![7, 7],
    };
    let outputs = PortDescriptions {
        buses: shape.outputs.to_vec(),
        native_ids: vec![8],
    };
    assert!(super::build_layout(None, duplicate_inputs, outputs).is_err());

    let invalid_input = PortDescriptions {
        buses: shape.inputs.to_vec(),
        native_ids: vec![u32::MAX],
    };
    let valid_output = PortDescriptions {
        buses: shape.outputs.to_vec(),
        native_ids: vec![8],
    };
    assert!(super::build_layout(None, invalid_input, valid_output).is_err());
}

#[test]
fn malformed_stereo_channel_count_is_rejected() {
    let mut malformed = port_info(b"Bad stereo", CLAP_AUDIO_PORT_IS_MAIN, 6);
    malformed.port_type = CLAP_PORT_STEREO.as_ptr();
    assert!(bus_from_info(&malformed, true, 0).is_err());
}

#[test]
fn configuration_summary_must_match_explicit_native_channel_type() {
    let layout = topology("Unspecified 2ch", BusKind::Main, ChannelConfig::Discrete(2));
    let mut config = super::empty_config();
    config.input_port_count = 1;
    config.output_port_count = 1;
    config.has_main_input = true;
    config.main_input_channel_count = 2;
    config.main_input_port_type = CLAP_PORT_STEREO.as_ptr();
    config.has_main_output = true;
    config.main_output_channel_count = 2;
    config.main_output_port_type = CLAP_PORT_STEREO.as_ptr();

    assert!(super::validate_config_summary(&config, &layout).is_err());
}
