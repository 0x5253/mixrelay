use std::sync::Arc;

use super::*;
use crate::{Control, Device, Kind, mailbox::InputSequence};

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

fn note(action: Action, number: u8) -> Binding {
    let mut binding = binding(action);
    binding.control.kind = Kind::Note;
    binding.control.number = number;
    binding
}

fn router(bindings: &[Binding]) -> Router {
    let mut router = Router::default();
    router.load(bindings);
    router
}

fn controller(mailbox: &Mailbox) -> [Source<'_>; 1] {
    [Source {
        device_id: "controller",
        mailbox,
    }]
}

/// Every target is ready at 0 dB unless the test names its own gains.
const AT_ZERO: fn(&Target) -> Option<f32> = |_| Some(0.0);

fn sent_actions(router: &mut Router) -> Vec<Action> {
    let mut sent = Vec::new();
    router.dispatch(AT_ZERO, || true, |binding| sent.push(binding.action));
    sent
}

#[test]
fn overflow_during_a_two_port_drain_preserves_transport_order() {
    let sequence = Arc::<InputSequence>::default();
    let first = Mailbox::new(Arc::clone(&sequence));
    let second = Mailbox::new(sequence);
    let record = note(Action::Record, 60);
    let mut stop = record.clone();
    stop.control.device.id = "second".into();
    stop.action = Action::Stop;
    let mut router = router(&[record, stop]);
    let sources = [
        Source {
            device_id: "controller",
            mailbox: &first,
        },
        Source {
            device_id: "second",
            mailbox: &second,
        },
    ];
    for _ in 0..1024 {
        first.receive(&[0x90, 60, 100]);
    }
    second.receive(&[0x90, 60, 100]);
    router.drain_with(sources, |id| {
        if id == "controller" {
            // The iterator has captured A's 0..1024 prefix. Replace only cursor 0.
            first.receive(&[0x90, 60, 100]);
        }
    });
    let sent = sent_actions(&mut router);
    assert_eq!(
        sent.len(),
        1024,
        "the retained prefix must precede the other port"
    );
    assert!(sent[..1023].iter().all(|action| *action == Action::Record));
    assert_eq!(sent[1023], Action::Stop);
    router.drain(sources);
    assert_eq!(sent_actions(&mut router), [Action::Record]);
    router.drain(sources);
    assert!(router.inputs.is_empty());
}

#[test]
fn a_power_or_mute_binding_cannot_consume_a_pending_volume_for_the_same_node() {
    let mut router = router(&[
        binding(Action::Power),
        binding(Action::Mute),
        binding(Action::Volume),
        binding(Action::Volume),
    ]);
    let target = router.bindings[0].target.clone();
    router.gains.push(target.clone(), -12.0);
    let mut sent = Vec::new();
    assert!(router.publish_gains(0, |binding, value| {
        sent.push((binding.action, value));
        Some((true, 100))
    }));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, Action::Volume);
    assert_eq!(sent[0].1.to_bits(), (-12.0_f32).to_bits());
    router.gains.push(target.clone(), -6.0);
    assert!(!router.publish_gains(133, |_, _| panic!("rate bound")));
    assert!(router.publish_gains(134, |_, _| Some((false, 134))));
    assert_eq!(router.gains.desired(&target), None);
}

#[test]
fn a_rejected_gain_requires_pickup_again_and_a_dropped_send_discards_pending_gains() {
    let mailbox = Mailbox::default();
    let mut router = router(&[binding(Action::Volume)]);
    // 106 maps to 0 dB, which picks up the fader at the observed level.
    mailbox.receive(&[0xb0, 7, 106]);
    router.drain(controller(&mailbox));
    router.dispatch(AT_ZERO, || true, |_| panic!("volume uses the coalescer"));
    assert!(router.publish_gains(0, |_, _| Some((false, 1))));
    // Pick-up was reset by the rejection, so a distant value no longer moves the target.
    mailbox.receive(&[0xb0, 7, 0]);
    router.drain(controller(&mailbox));
    router.dispatch(AT_ZERO, || true, |_| panic!("volume uses the coalescer"));
    assert!(!router.publish_gains(100, |_, _| panic!("no gain after lost pick-up")));

    let target = router.bindings[0].target.clone();
    router.gains.push(target.clone(), -3.0);
    assert!(!router.publish_gains(200, |_, _| None));
    assert_eq!(router.gains.desired(&target), None);
}

#[test]
fn loading_a_new_mapping_discards_pending_gain_and_requires_pickup_again() {
    let mailbox = Mailbox::default();
    let bindings = [binding(Action::Volume)];
    let mut router = router(&bindings);
    mailbox.receive(&[0xb0, 7, 106]);
    router.drain(controller(&mailbox));
    router.dispatch(AT_ZERO, || true, |_| {});
    let target = bindings[0].target.clone();
    assert_eq!(router.gains.desired(&target), Some(0.0));

    router.load(&bindings);
    assert_eq!(router.gains.desired(&target), None);
    mailbox.receive(&[0xb0, 7, 0]);
    router.drain(controller(&mailbox));
    router.dispatch(AT_ZERO, || true, |_| {});
    assert_eq!(router.gains.desired(&target), None);
}

#[test]
fn releases_and_repeated_presses_preserve_transport_order_across_controls() {
    let mailbox = Mailbox::default();
    let mut router = router(&[note(Action::Stop, 61), note(Action::Record, 60)]);
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
        router.drain(controller(&mailbox));
        assert_eq!(sent_actions(&mut router), expected);
    }
}

#[test]
fn returning_an_absolute_fader_to_observed_gain_replaces_older_throttled_intent() {
    let mailbox = Mailbox::default();
    let mut router = router(&[binding(Action::Volume)]);
    let target = router.bindings[0].target.clone();
    router.gains.push(target.clone(), 0.0);
    assert_eq!(router.gains.take(&target, 0), Some(0.0));
    for value in [106, 120, 106] {
        mailbox.receive(&[0xb0, 7, value]);
        router.drain(controller(&mailbox));
        router.dispatch(AT_ZERO, || true, |_| panic!("volume uses the coalescer"));
    }
    assert_eq!(router.gains.take(&target, 33), None);
    assert_eq!(router.gains.take(&target, 34), Some(0.0));
}

#[test]
fn a_target_that_is_not_ready_resets_pickup_instead_of_queueing_a_gain() {
    let mailbox = Mailbox::default();
    let mut router = router(&[binding(Action::Volume)]);
    let target = router.bindings[0].target.clone();
    mailbox.receive(&[0xb0, 7, 106]);
    router.drain(controller(&mailbox));
    router.dispatch(|_| None, || true, |_| {});
    assert_eq!(router.gains.desired(&target), None);
}

#[test]
fn a_dispatch_that_ends_admission_drops_the_rest_of_the_batch_and_pending_gains() {
    let mailbox = Mailbox::default();
    let mut later = binding(Action::Record);
    later.control.number = 8;
    let mut router = router(&[binding(Action::Mute), later]);
    for bytes in [[0xb0, 7, 127], [0xb0, 7, 0], [0xb0, 7, 127], [0xb0, 8, 127]] {
        mailbox.receive(&bytes);
    }
    router.drain(controller(&mailbox));
    let pending = router.bindings[1].target.clone();
    router.gains.push(pending.clone(), 8.0);
    let admitted = std::cell::Cell::new(true);
    let mut sent = Vec::new();
    router.dispatch(
        AT_ZERO,
        || admitted.get(),
        |binding| {
            sent.push(binding.action);
            admitted.set(false);
        },
    );
    assert_eq!(sent, [Action::Mute]);
    assert_eq!(router.gains.desired(&pending), None);
}

#[test]
fn one_cc_batch_fans_out_to_every_target_with_independent_pickup_and_steps() {
    let mailbox = Mailbox::default();
    let first = binding(Action::Volume);
    let mut second = first.clone();
    second.target.node_id = "second".into();
    let mut router = router(&[first, second]);
    let gains = |target: &Target| Some(if target.node_id == "gain" { 0.0 } else { -60.0 });
    mailbox.receive(&[0xb0, 7, 0]);
    router.drain(controller(&mailbox));
    assert_eq!(router.inputs.len(), 2);
    router.dispatch(gains, || true, |_| panic!("volume"));
    let [first, second] = [0, 1].map(|index| router.bindings[index].target.clone());
    assert_eq!(router.gains.desired(&first), None);
    assert_eq!(router.gains.desired(&second), Some(-60.0));

    let mut relative = router.bindings.clone();
    relative[0].mode = Mode::TwosComplement;
    relative[1].mode = Mode::TwosComplement;
    relative[1].step_db = 3.0;
    // Keep pick-up and gains; only the encoder options change.
    router.bindings = relative;
    mailbox.receive(&[0xb0, 7, 1]);
    router.drain(controller(&mailbox));
    router.dispatch(gains, || true, |_| panic!("volume"));
    assert_eq!(router.gains.take(&first, 0), Some(1.0));
    assert_eq!(router.gains.take(&second, 0), Some(-57.0));
    router.gains.push(first.clone(), 2.0);
    router.gains.push(second.clone(), -54.0);
    assert_eq!(router.gains.take(&first, 33), None);
    assert_eq!(router.gains.take(&second, 33), None);
    assert_eq!(router.gains.take(&first, 34), Some(2.0));
    assert_eq!(router.gains.take(&second, 34), Some(-54.0));
}

#[test]
fn each_press_fans_out_before_the_next_press_and_nothing_runs_without_admission() {
    let mailbox = Mailbox::default();
    let first = binding(Action::Mute);
    let mut second = first.clone();
    second.target.node_id = "second".into();
    let mut router = router(&[first, second]);
    for bytes in [[0xb0, 7, 127], [0xb0, 7, 0], [0xb0, 7, 127]] {
        mailbox.receive(&bytes);
    }
    router.drain(controller(&mailbox));
    assert_eq!(
        router
            .inputs
            .iter()
            .map(|(index, _)| *index)
            .collect::<Vec<_>>(),
        [0, 1, 0, 1]
    );
    router.dispatch(AT_ZERO, || false, |_| panic!("not admitted"));
}

#[test]
fn a_port_still_publishing_defers_the_whole_drain() {
    let mailbox = Mailbox::default();
    let mut router = router(&[note(Action::Record, 60)]);
    let publication = mailbox.begin();
    router.drain(controller(&mailbox));
    assert!(router.inputs.is_empty());
    publication.receive(&[0x90, 60, 100]);
    router.drain(controller(&mailbox));
    assert_eq!(sent_actions(&mut router), [Action::Record]);
}

#[test]
fn bindings_on_another_device_ignore_this_ports_input() {
    let mailbox = Mailbox::default();
    let mut elsewhere = note(Action::Record, 60);
    elsewhere.control.device.id = "other".into();
    let mut router = router(&[elsewhere]);
    mailbox.receive(&[0x90, 60, 100]);
    router.drain(controller(&mailbox));
    assert!(router.inputs.is_empty());
}
