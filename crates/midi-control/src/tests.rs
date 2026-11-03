use super::*;

pub(crate) fn binding() -> Binding {
    Binding {
        control: Control {
            device: Device {
                id: "uid:42".into(),
                name: "Fader".into(),
            },
            channel: 0,
            kind: Kind::Cc,
            number: 7,
        },
        target: Target {
            graph_id: "main".into(),
            node_id: "gain".into(),
        },
        action: Action::Volume,
        mode: Mode::Absolute,
        step_db: 1.0,
        graph_name: "Programme".into(),
        node_name: "Microphone".into(),
        min_db: -60.0,
        max_db: 12.0,
    }
}

#[test]
fn matching_uses_port_channel_kind_and_number_but_not_display_name() {
    let control = binding().control;
    let mut renamed = control.clone();
    renamed.device.name = "Renamed".into();
    assert!(control.matches(&renamed));
    for change in 0..4 {
        let mut different = control.clone();
        match change {
            0 => different.device.id = "other".into(),
            1 => different.channel = 1,
            2 => different.kind = Kind::Note,
            _ => different.number = 8,
        }
        assert!(!control.matches(&different));
    }
}

#[test]
fn one_control_drives_distinct_targets_and_actions_and_duplicates_keep_options() {
    let mut bindings = Vec::new();
    assign(&mut bindings, binding()).unwrap();
    let mut second = binding();
    second.target.node_id = "second".into();
    second.mode = Mode::Offset64;
    assign(&mut bindings, second.clone()).unwrap();
    second.action = Action::Mute;
    assign(&mut bindings, second).unwrap();
    let mut duplicate = binding();
    duplicate.step_db = 3.0;
    assign(&mut bindings, duplicate).unwrap();
    assert_eq!(bindings.len(), 3);
    assert_eq!(bindings[0].step_db.to_bits(), 1.0_f32.to_bits());
    assert_eq!(bindings[1].mode, Mode::Offset64);
    let mut another_input = binding();
    another_input.control.number = 8;
    assign(&mut bindings, another_input).unwrap();
    assert_eq!(bindings.len(), 4);
}

#[test]
fn untrusted_preferences_validate_and_round_trip_without_losing_missing_devices_or_targets() {
    let entries = vec![binding()];
    let bytes = serde_json::to_vec(&entries).unwrap();
    assert_eq!(
        serde_json::from_slice::<Vec<Binding>>(&bytes).unwrap(),
        entries
    );
    assert_eq!(binding().validate(), Ok(()));
    for change in 0..12 {
        let mut invalid = binding();
        match change {
            0 => invalid.control.channel = 16,
            1 => invalid.control.number = 128,
            2 => invalid.control.device.id.clear(),
            3 => invalid.control.device.name = "a".repeat(1025),
            4 => invalid.target.graph_id = "\0".into(),
            5 => invalid.step_db = f32::NAN,
            6 => invalid.control.kind = Kind::Note,
            7 => invalid.target.node_id.clear(),
            8 => invalid.min_db = f32::NAN,
            9 => invalid.max_db = f32::INFINITY,
            10 => invalid.min_db = -61.0,
            _ => invalid.max_db = 13.0,
        }
        assert_eq!(assign(&mut vec![], invalid), Err(BindingError::Invalid));
    }
    let mut full = Vec::new();
    for number in 0..257 {
        let mut entry = binding();
        entry.control.device.id = number.to_string();
        assign(&mut full, entry).unwrap();
    }
    assign(&mut full, binding()).unwrap();
    assert_eq!(full.len(), 258);
    assert_eq!(BindingError::Invalid.to_string(), "invalid MIDI binding");
}
