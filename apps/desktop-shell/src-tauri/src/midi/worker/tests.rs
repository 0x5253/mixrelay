use super::*;
use crate::midi::Learn;
use mixrelay_midi_control::{Control, Device, Kind, Target};

#[test]
fn overflow_during_a_two_port_drain_preserves_transport_order() {
    let sequence = Arc::default();
    let first = Arc::new(mixrelay_midi_control::mailbox::Mailbox::new(Arc::clone(
        &sequence,
    )));
    let second = Arc::new(mixrelay_midi_control::mailbox::Mailbox::new(sequence));
    let mut record = binding(Action::Record);
    record.control.kind = Kind::Note;
    record.control.number = 60;
    let mut stop = record.clone();
    stop.control.device.id = "second".into();
    stop.action = Action::Stop;
    let bindings = [record, stop];
    let mut ports = port(Arc::clone(&first));
    ports.insert(
        "second".into(),
        native::port_fixture::port(bindings[1].control.device.clone(), Arc::clone(&second)),
    );
    for _ in 0..1024 {
        first.receive(&[0x90, 60, 100]);
    }
    second.receive(&[0x90, 60, 100]);
    let state = Mutex::new(crate::midi::tests::memory_state());
    let mut pickup = [Pickup::default(), Pickup::default()];
    let mut gains = Coalescer::default();
    let mut inputs = Vec::new();
    let mut sent = Vec::new();
    drain_inputs_with(&ports, &bindings, &mut inputs, |id| {
        if id == "controller" {
            // The iterator has captured A's 0..1024 prefix. Replace only cursor 0.
            first.receive(&[0x90, 60, 100]);
        }
    });
    process_inputs(
        (&bindings, &mut pickup),
        &inputs,
        (&mut gains, &serde_json::Value::Null),
        (&state, 0),
        |binding| sent.push(binding.action),
    );
    assert_eq!(
        sent.len(),
        1024,
        "the retained prefix must precede the other port"
    );
    assert!(sent[..1023].iter().all(|action| *action == Action::Record));
    assert_eq!(sent[1023], Action::Stop);
    drain_inputs(&ports, &bindings, &mut inputs);
    process_inputs(
        (&bindings, &mut pickup),
        &inputs,
        (&mut gains, &serde_json::Value::Null),
        (&state, 0),
        |binding| sent.push(binding.action),
    );
    assert_eq!(sent.len(), 1025);
    assert_eq!(sent[1024], Action::Record);
    drain_inputs(&ports, &bindings, &mut inputs);
    assert!(inputs.is_empty());
}

fn binding(action: Action) -> Binding {
    Binding {
        control: Control {
            device: Device {
                id: "controller".into(),
                name: "Controller".into(),
            },
            channel: 0,
            kind: Kind::Cc,
            number: 7,
        },
        target: Target {
            graph_id: "main".into(),
            node_id: "gain".into(),
        },
        action,
        mode: Mode::Absolute,
        step_db: 1.0,
        graph_name: "Programme".into(),
        node_name: "Microphone".into(),
        min_db: -60.0,
        max_db: 12.0,
    }
}

#[test]
fn a_power_or_mute_binding_cannot_consume_a_pending_volume_for_the_same_node() {
    let bindings = [
        binding(Action::Power),
        binding(Action::Mute),
        binding(Action::Volume),
        binding(Action::Volume),
    ];
    let mut pickup: Vec<_> = bindings.iter().map(|_| Pickup::default()).collect();
    let mut gains = Coalescer::default();
    gains.push(bindings[0].target.clone(), -12.0);
    let mut sent = Vec::new();
    assert!(publish_gains(
        &bindings,
        &mut pickup,
        &mut gains,
        0,
        |binding, value| {
            sent.push((binding.action, value));
            Some((true, 100))
        }
    ));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, Action::Volume);
    assert_eq!(sent[0].1.to_bits(), (-12.0_f32).to_bits());
    gains.push(bindings[0].target.clone(), -6.0);
    assert!(!publish_gains(
        &bindings,
        &mut pickup,
        &mut gains,
        133,
        |_, _| panic!("rate bound")
    ));
    assert!(publish_gains(
        &bindings,
        &mut pickup,
        &mut gains,
        134,
        |_, _| Some((false, 134))
    ));
    assert_eq!(gains.desired(&bindings[0].target), None);
}

#[test]
fn cancelling_learn_discards_pending_gain_and_requires_pickup_again() {
    let state = Mutex::new(crate::midi::tests::memory_state());
    lock(&state)
        .store
        .save(vec![binding(Action::Volume)])
        .unwrap();
    let mut generation = u64::MAX;
    let mut bindings = Vec::new();
    let mut pickup = Vec::new();
    let mut gains = Coalescer::default();
    assert!(!refresh_settings(
        &state,
        &mut generation,
        &mut bindings,
        &mut pickup,
        &mut gains
    ));
    let mailbox = mixrelay_midi_control::mailbox::Mailbox::default();
    mailbox.receive(&[0xb0, 7, 106]);
    assert!(
        pickup[0]
            .map(&bindings[0], mailbox.take(0, Kind::Cc, 7).unwrap(), 0.0)
            .is_some()
    );
    gains.push(bindings[0].target.clone(), 6.0);
    lock(&state).clear_input();
    assert!(!refresh_settings(
        &state,
        &mut generation,
        &mut bindings,
        &mut pickup,
        &mut gains
    ));
    assert_eq!(gains.desired(&bindings[0].target), None);
    mailbox.receive(&[0xb0, 7, 0]);
    assert_eq!(
        pickup[0].map(&bindings[0], mailbox.take(0, Kind::Cc, 7).unwrap(), 0.0),
        None
    );
}

fn port(mailbox: Arc<mixrelay_midi_control::mailbox::Mailbox>) -> BTreeMap<String, Port<()>> {
    BTreeMap::from([(
        "controller".into(),
        native::port_fixture::port(
            Device {
                id: "controller".into(),
                name: "Controller".into(),
            },
            mailbox,
        ),
    )])
}

#[test]
fn releases_and_repeated_presses_preserve_transport_order_across_controls() {
    let mailbox = Arc::new(mixrelay_midi_control::mailbox::Mailbox::default());
    let mut record = binding(Action::Record);
    record.control.kind = Kind::Note;
    record.control.number = 60;
    let mut stop = record.clone();
    stop.control.number = 61;
    stop.action = Action::Stop;
    let bindings = [stop, record];
    let ports = port(Arc::clone(&mailbox));
    for (messages, expected) in [
        (
            vec![[0x90, 60, 100], [0x90, 61, 100], [0x80, 60, 0]],
            vec![Action::Record, Action::Stop],
        ),
        (
            vec![[0x90, 60, 100], [0x90, 61, 100], [0x90, 60, 100]],
            vec![Action::Record, Action::Stop, Action::Record],
        ),
    ] {
        for bytes in messages {
            mailbox.receive(&bytes);
        }
        let mut inputs = Vec::new();
        drain_inputs(&ports, &bindings, &mut inputs);
        let actual: Vec<_> = inputs
            .iter()
            .flat_map(|(index, input)| {
                std::iter::repeat_n(bindings[*index].action, usize::from(input.presses))
            })
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn returning_an_absolute_fader_to_observed_gain_replaces_older_throttled_intent() {
    let binding = binding(Action::Volume);
    let mailbox = mixrelay_midi_control::mailbox::Mailbox::default();
    let mut pickup = Pickup::default();
    let mut gains = Coalescer::default();
    let snapshot = serde_json::json!({"revision":1,"activeRevision":1,"enginePower":{"enabled":true,"transitioning":false},"graphs":[{"id":"main","nodes":[{"nodeId":"gain","gainDb":0,"control":{"enabled":true}}]}]});
    gains.push(binding.target.clone(), 0.0);
    assert_eq!(gains.take(&binding.target, 0), Some(0.0));
    for value in [106, 120, 106] {
        mailbox.receive(&[0xb0, 7, value]);
        apply(
            (&binding, &mut pickup),
            mailbox.take(0, Kind::Cc, 7).unwrap(),
            &mut gains,
            &snapshot,
            |_| panic!("volume uses coalescer"),
        );
    }
    assert_eq!(gains.take(&binding.target, 33), None);
    assert_eq!(gains.take(&binding.target, 34), Some(0.0));
}

#[test]
fn drained_actions_are_discarded_when_a_dispatch_changes_bindings_or_learn() {
    for change in 0..4 {
        let state = Mutex::new(crate::midi::tests::memory_state());
        let first = binding(Action::Mute);
        let mut later = binding(Action::Record);
        later.control.number = 8;
        let bindings = [first, later];
        lock(&state).store.save(bindings.to_vec()).unwrap();
        let generation = lock(&state).store.generation();
        let mailbox = mixrelay_midi_control::mailbox::Mailbox::default();
        for bytes in [[0xb0, 7, 127], [0xb0, 7, 0], [0xb0, 7, 127], [0xb0, 8, 127]] {
            mailbox.receive(&bytes);
        }
        let inputs = [
            (0, mailbox.take(0, Kind::Cc, 7).unwrap()),
            (1, mailbox.take(0, Kind::Cc, 8).unwrap()),
        ];
        let mut pickup = [Pickup::default(), Pickup::default()];
        let mut gains = Coalescer::default();
        gains.push(bindings[1].target.clone(), 8.0);
        let mut sent = Vec::new();
        process_inputs(
            (&bindings, &mut pickup),
            &inputs,
            (&mut gains, &serde_json::Value::Null),
            (&state, generation),
            |binding| {
                sent.push(binding.action);
                let mut state = lock(&state);
                match change {
                    0 => {
                        state.store.save(vec![]).unwrap();
                    }
                    1 => {
                        let mut replaced = bindings[1].clone();
                        replaced.action = Action::Stop;
                        state.store.save(vec![replaced]).unwrap();
                    }
                    2 => {
                        state.clear_input();
                        state.learn = Learn::Idle;
                        state.map_mode = false;
                    }
                    _ => {
                        state.learn = Learn::Waiting;
                        state.map_mode = true;
                    }
                }
            },
        );
        assert_eq!(sent, vec![Action::Mute], "change {change}");
        assert_eq!(gains.desired(&bindings[1].target), None);
    }
}

#[test]
fn one_cc_batch_fans_out_to_every_target_with_independent_pickup_and_steps() {
    let mailbox = Arc::new(mixrelay_midi_control::mailbox::Mailbox::default());
    let ports = port(Arc::clone(&mailbox));
    let first = binding(Action::Volume);
    let mut second = first.clone();
    second.target.node_id = "second".into();
    let bindings = [first, second];
    let mut pickup = [Pickup::default(), Pickup::default()];
    let mut gains = Coalescer::default();
    let snapshot = serde_json::json!({"revision":1,"activeRevision":1,"enginePower":{"enabled":true,"transitioning":false},"graphs":[{"id":"main","nodes":[{"nodeId":"gain","gainDb":0,"control":{"enabled":true}},{"nodeId":"second","gainDb":-60,"control":{"enabled":true}}]}]});
    mailbox.receive(&[0xb0, 7, 0]);
    let mut inputs = Vec::new();
    drain_inputs(&ports, &bindings, &mut inputs);
    assert_eq!(inputs.len(), 2);
    for (index, input) in inputs {
        apply(
            (&bindings[index], &mut pickup[index]),
            input,
            &mut gains,
            &snapshot,
            |_| panic!("volume"),
        );
    }
    assert_eq!(gains.desired(&bindings[0].target), None);
    assert_eq!(gains.desired(&bindings[1].target), Some(-60.0));
    let mut relative = bindings;
    relative[0].mode = Mode::TwosComplement;
    relative[1].mode = Mode::TwosComplement;
    relative[1].step_db = 3.0;
    mailbox.receive(&[0xb0, 7, 1]);
    let mut inputs = Vec::new();
    drain_inputs(&ports, &relative, &mut inputs);
    for (index, input) in inputs {
        apply(
            (&relative[index], &mut pickup[index]),
            input,
            &mut gains,
            &snapshot,
            |_| panic!("volume"),
        );
    }
    assert_eq!(gains.take(&relative[0].target, 0), Some(1.0));
    assert_eq!(gains.take(&relative[1].target, 0), Some(-57.0));
    gains.push(relative[0].target.clone(), 2.0);
    gains.push(relative[1].target.clone(), -54.0);
    assert_eq!(gains.take(&relative[0].target, 33), None);
    assert_eq!(gains.take(&relative[1].target, 33), None);
    assert_eq!(gains.take(&relative[0].target, 34), Some(2.0));
    assert_eq!(gains.take(&relative[1].target, 34), Some(-54.0));
}

#[test]
fn each_press_fans_out_before_the_next_press_and_map_mode_without_a_target_runs_nothing() {
    let state = Mutex::new(crate::midi::tests::memory_state());
    let mailbox = Arc::new(mixrelay_midi_control::mailbox::Mailbox::default());
    let ports = port(Arc::clone(&mailbox));
    let first = binding(Action::Mute);
    let mut second = first.clone();
    second.target.node_id = "second".into();
    let bindings = [first, second];
    for bytes in [[0xb0, 7, 127], [0xb0, 7, 0], [0xb0, 7, 127]] {
        mailbox.receive(&bytes);
    }
    let mut inputs = Vec::new();
    drain_inputs(&ports, &bindings, &mut inputs);
    assert_eq!(
        inputs.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    let mut pickup = [Pickup::default(), Pickup::default()];
    let mut gains = Coalescer::default();
    let mut sent = Vec::new();
    lock(&state).map_mode = true;
    process_inputs(
        (&bindings, &mut pickup),
        &inputs,
        (&mut gains, &serde_json::Value::Null),
        (&state, 0),
        |_| sent.push(1),
    );
    assert!(sent.is_empty());
}
