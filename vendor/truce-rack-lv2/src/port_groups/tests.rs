use super::*;
use crate::Lv2Scanner;
use truce_rack_core::buffer::{AudioBuffer, BusRange};
use truce_rack_core::events::EventList;
use truce_rack_core::plugin::{Plugin, PluginCore, ProcessContext};
use truce_rack_core::scanner::PluginScanner;

fn fixture() -> (Lv2Scanner, truce_rack_core::info::PluginInfo) {
    let scanner = Lv2Scanner::new();
    let info = scanner
        .scan()
        .expect("scan isolated LV2_PATH")
        .into_iter()
        .find(|info| info.unique_id == "urn:mixrelay:multibus-fixture")
        .expect("compile fixtures/multibus.lv2 and set isolated LV2_PATH");
    (scanner, info)
}

fn run_block(
    plugin: &mut crate::Lv2Plugin,
    input: &[[f32; 32]; 3],
    output: &mut [[f32; 32]; 3],
    events: &EventList,
    sink: &mut EventList,
) {
    let input_refs = [
        input[0].as_slice(),
        input[1].as_slice(),
        input[2].as_slice(),
    ];
    let [out0, out1, out2] = output;
    let mut output_refs = [
        out0.as_mut_slice(),
        out1.as_mut_slice(),
        out2.as_mut_slice(),
    ];
    let input_ranges = [BusRange::new(0, 2), BusRange::new(2, 1)];
    let output_ranges = [BusRange::new(0, 2), BusRange::new(2, 1)];
    let mut buffer = AudioBuffer::new(
        &input_refs,
        &mut output_refs,
        32,
        &input_ranges,
        &output_ranges,
    );
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: 32,
        transport: None,
        output_events: sink,
    };
    plugin.process(&mut buffer, events, &mut context).unwrap();
}

#[test]
fn explicit_main_keeps_ungrouped_audio_as_auxiliary_without_guessing_channels() {
    let mut ports = [
        PortInfo {
            index: 0,
            kind: PortKind::Audio,
            is_input: true,
            audio_plane: None,
        },
        PortInfo {
            index: 1,
            kind: PortKind::Audio,
            is_input: true,
            audio_plane: None,
        },
    ];
    let make_groups = || {
        vec![
            AudioGroup {
                uri: None,
                name: String::new(),
                side_of: None,
                members: vec![Member {
                    port: 0,
                    symbol: "other".into(),
                    designation: None,
                }],
            },
            AudioGroup {
                uri: Some("urn:test:main".into()),
                name: "main".into(),
                side_of: None,
                members: vec![Member {
                    port: 1,
                    symbol: "main".into(),
                    designation: None,
                }],
            },
        ]
    };
    let metadata = HashMap::new();
    assert!(finish_direction(&mut make_groups(), None, true, &mut ports, &metadata).is_err());
    let result = finish_direction(
        &mut make_groups(),
        Some("urn:test:main"),
        true,
        &mut ports,
        &metadata,
    )
    .unwrap();
    assert_eq!(result.buses[0].kind, BusKind::Main);
    assert_eq!(result.buses[1].kind, BusKind::Auxiliary);
    assert_eq!(ports[0].audio_plane, Some(1));
    assert_eq!(ports[1].audio_plane, Some(0));
}

#[test]
fn declared_element_indices_reorder_members_without_using_native_port_position() {
    let uri = "urn:test:indexed".to_owned();
    let mut metadata = HashMap::new();
    metadata.insert(
        uri.clone(),
        GroupMeta {
            uri: uri.clone(),
            symbol: "indexed".into(),
            side_of: None,
            direction: Some(true),
            elements: vec![
                Element {
                    designation: "urn:test:right".into(),
                    index: Some(0),
                },
                Element {
                    designation: "urn:test:left".into(),
                    index: Some(1),
                },
            ],
        },
    );
    let mut groups = vec![AudioGroup {
        uri: Some(uri.clone()),
        name: "indexed".into(),
        side_of: None,
        members: vec![
            Member {
                port: 0,
                symbol: "left".into(),
                designation: Some("urn:test:left".into()),
            },
            Member {
                port: 1,
                symbol: "right".into(),
                designation: Some("urn:test:right".into()),
            },
        ],
    }];
    let mut ports = [0, 1].map(|index| PortInfo {
        index,
        kind: PortKind::Audio,
        is_input: true,
        audio_plane: None,
    });
    let result = finish_direction(&mut groups, Some(&uri), true, &mut ports, &metadata).unwrap();
    assert_eq!(result.buses[0].channels, ChannelConfig::Discrete(2));
    assert_eq!(ports[0].audio_plane, Some(1));
    assert_eq!(ports[1].audio_plane, Some(0));
}

#[test]
fn canonical_identity_accepts_sixty_four_long_native_members_each_direction() {
    let mut ports: Vec<PortInfo> = (0..128_u32)
        .map(|index| PortInfo {
            index,
            kind: PortKind::Audio,
            is_input: index < 64,
            audio_plane: None,
        })
        .collect();
    let make_group = |direction: &str, offset: usize| AudioGroup {
        uri: Some(format!("urn:mixrelay:{}:{}", direction, "long".repeat(100))),
        name: format!("{}_{}", direction, "long".repeat(100)),
        side_of: None,
        members: (offset..offset + 64)
            .map(|index| Member {
                port: index,
                symbol: format!("{}_{}_{}", direction, index, "long".repeat(20)),
                designation: None,
            })
            .collect(),
    };
    let mut inputs = vec![make_group("input", 0)];
    let mut outputs = vec![make_group("output", 64)];
    let metadata = HashMap::new();
    let input = finish_direction(&mut inputs, None, true, &mut ports, &metadata).unwrap();
    let output = finish_direction(&mut outputs, None, false, &mut ports, &metadata).unwrap();
    assert_eq!(input.buses[0].channels, ChannelConfig::Discrete(64));
    assert_eq!(output.buses[0].channels, ChannelConfig::Discrete(64));
    assert_eq!(ports[63].audio_plane, Some(63));
    assert_eq!(ports[127].audio_plane, Some(63));
    let id = layout_id(input.identity, output.identity).unwrap();
    assert_eq!(id.len(), "lv2-pg-v1:".len() + 64);
    inputs[0].members.push(Member {
        port: 0,
        symbol: "extra".into(),
        designation: None,
    });
    assert!(finish_direction(&mut inputs, None, true, &mut ports, &metadata).is_err());
}

/// Requires one compiled repository-owned bundle and a process-local isolated `LV2_PATH`.
#[test]
#[ignore = "requires compiled fixtures/multibus.lv2 and isolated LV2_PATH"]
fn native_group_permutation_and_fresh_state_restore() {
    let (scanner, info) = fixture();
    let mut plugin = scanner.load(&info).unwrap();
    let layout = plugin.supported_layouts()[0].clone();
    assert_eq!(layout.inputs.len(), 2);
    assert_eq!(layout.outputs.len(), 2);
    assert_eq!(layout.inputs[0].kind, BusKind::Main);
    assert_eq!(layout.inputs[0].channels.count(), 2);
    assert_eq!(layout.inputs[1].kind, BusKind::Sidechain);
    assert_eq!(layout.outputs[0].kind, BusKind::Main);
    assert_eq!(layout.outputs[0].channels.count(), 2);
    assert_eq!(layout.outputs[1].kind, BusKind::Auxiliary);
    assert_eq!(
        plugin
            .bindings
            .ports
            .iter()
            .filter_map(|p| p.audio_plane)
            .collect::<Vec<_>>(),
        [0, 2, 1, 0, 2, 1]
    );
    plugin.activate(layout.clone(), 48_000.0, 32).unwrap();
    let input = [[1.0; 32], [2.0; 32], [3.0; 32]];
    let mut output = [[0.0; 32]; 3];
    let events = EventList::new();
    let mut sink = EventList::new();
    run_block(&mut plugin, &input, &mut output, &events, &mut sink);
    assert!(
        output[0]
            .iter()
            .all(|sample| sample.to_bits() == 32.0_f32.to_bits())
    );
    assert!(
        output[1]
            .iter()
            .all(|sample| sample.to_bits() == 63.0_f32.to_bits())
    );
    assert!(
        output[2]
            .iter()
            .all(|sample| sample.to_bits() == 94.0_f32.to_bits())
    );
    let state = plugin.save_state().unwrap();
    assert_wrong_bus_topology_does_not_run(&mut plugin, &state);
    drop(plugin);

    let mut fresh = scanner.load(&info).unwrap();
    assert_eq!(fresh.supported_layouts()[0], layout);
    fresh
        .activate_with_state(layout, 48_000.0, 32, Some(&state))
        .unwrap();
    assert_eq!(fresh.save_state().unwrap(), state);
    output = [[0.0; 32]; 3];
    run_block(&mut fresh, &input, &mut output, &events, &mut sink);
    assert!(
        output[0]
            .iter()
            .all(|sample| sample.to_bits() == 33.0_f32.to_bits())
    );
    assert!(
        output[1]
            .iter()
            .all(|sample| sample.to_bits() == 64.0_f32.to_bits())
    );
    assert!(
        output[2]
            .iter()
            .all(|sample| sample.to_bits() == 95.0_f32.to_bits())
    );

    assert_other_bundle_layouts(&scanner);
}

fn assert_wrong_bus_topology_does_not_run(plugin: &mut crate::Lv2Plugin, state: &[u8]) {
    let input = [[1.0; 32], [2.0; 32], [3.0; 32]];
    let mut output = [[0.0; 32]; 3];
    let events = EventList::new();
    let mut sink = EventList::new();
    let input_refs = [
        input[0].as_slice(),
        input[1].as_slice(),
        input[2].as_slice(),
    ];
    let [out0, out1, out2] = &mut output;
    let mut output_refs = [
        out0.as_mut_slice(),
        out1.as_mut_slice(),
        out2.as_mut_slice(),
    ];
    let wrong_input_buses = [BusRange::new(0, 1), BusRange::new(1, 2)];
    let output_buses = [BusRange::new(0, 2), BusRange::new(2, 1)];
    let mut wrong_buffer = AudioBuffer::new(
        &input_refs,
        &mut output_refs,
        32,
        &wrong_input_buses,
        &output_buses,
    );
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: 32,
        transport: None,
        output_events: &mut sink,
    };
    assert!(
        plugin
            .process(&mut wrong_buffer, &events, &mut context)
            .is_err()
    );
    assert_eq!(
        plugin.save_state().unwrap().as_slice(),
        state,
        "invalid topology must not run native DSP"
    );
}

fn assert_other_bundle_layouts(scanner: &Lv2Scanner) {
    let indexed_info = scanner
        .scan()
        .unwrap()
        .into_iter()
        .find(|info| info.unique_id == "urn:mixrelay:multibus-indexed")
        .unwrap();
    let indexed = scanner.load(&indexed_info).unwrap();
    assert_eq!(
        indexed
            .bindings
            .ports
            .iter()
            .filter_map(|port| port.audio_plane)
            .collect::<Vec<_>>(),
        [1, 0, 0]
    );

    for (uri, expected) in [
        ("urn:mixrelay:multibus-ambiguous", "ambiguous"),
        ("urn:mixrelay:multibus-conflict", "mixes input and output"),
        (
            "urn:mixrelay:multibus-type-conflict",
            "conflicts with audio direction",
        ),
    ] {
        let bad = scanner
            .scan()
            .unwrap()
            .into_iter()
            .find(|i| i.unique_id == uri)
            .unwrap();
        assert!(
            scanner
                .load(&bad)
                .err()
                .expect("malformed LV2 metadata must fail before instantiate")
                .to_string()
                .contains(expected)
        );
    }
}

/// Same native instance, 32-frame block, and 16-block warmup before and after the pointer fix.
#[test]
#[ignore = "requires compiled fixtures/multibus.lv2 and isolated LV2_PATH"]
fn native_process_pointer_allocation_count() {
    let (scanner, info) = fixture();
    let mut plugin = scanner.load(&info).unwrap();
    let layout = plugin.supported_layouts()[0].clone();
    plugin.activate(layout, 48_000.0, 32).unwrap();
    let input = [[1.0; 32], [2.0; 32], [3.0; 32]];
    let mut output = [[0.0; 32]; 3];
    let events = EventList::new();
    let mut sink = EventList::new();
    for _ in 0..16 {
        run_block(&mut plugin, &input, &mut output, &events, &mut sink);
    }
    let allocations = crate::test_allocations::measure(|| {
        for _ in 0..100 {
            run_block(&mut plugin, &input, &mut output, &events, &mut sink);
        }
    });
    eprintln!("LV2 native process pointer allocations: {allocations}/100 blocks");
    assert_eq!(allocations, 0);
}

#[test]
#[ignore = "requires compiled fixtures/state.lv2 and isolated LV2_PATH"]
fn native_pending_controls_process_without_allocations() {
    let scanner = Lv2Scanner::new();
    let info = scanner
        .scan()
        .expect("scan isolated LV2_PATH")
        .into_iter()
        .find(|info| info.unique_id == "urn:mixrelay:state-fixture")
        .expect("compile fixtures/state.lv2 and set isolated LV2_PATH");
    let mut plugin = scanner.load(&info).unwrap();
    let layout = plugin.supported_layouts()[0].clone();
    plugin.activate(layout, 48_000.0, 32).unwrap();
    let input = [1.0; 32];
    let mut output = [0.0; 32];
    let events = EventList::new();
    let mut sink = EventList::new();
    let mut run = |gain: f32| {
        plugin.ui.controller.pending.publish(2, gain);
        let input_refs = [input.as_slice()];
        let mut output_refs = [output.as_mut_slice()];
        let ranges = [BusRange::new(0, 1)];
        let mut buffer = AudioBuffer::new(&input_refs, &mut output_refs, 32, &ranges, &ranges);
        let mut context = ProcessContext {
            sample_rate: 48_000.0,
            max_block_size: 32,
            transport: None,
            output_events: &mut sink,
        };
        plugin.process(&mut buffer, &events, &mut context).unwrap();
        assert!(
            output
                .iter()
                .all(|sample| (*sample - gain).abs() < f32::EPSILON)
        );
    };
    for _ in 0..16 {
        run(0.5);
    }
    let allocations = crate::test_allocations::measure(|| {
        for block in 0..100 {
            run(if block % 2 == 0 { 0.25 } else { 0.75 });
        }
    });
    eprintln!("LV2 pending control process allocations: {allocations}/100 blocks");
    assert_eq!(allocations, 0);
}
