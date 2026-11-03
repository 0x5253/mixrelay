#![expect(
    clippy::float_cmp,
    reason = "half-decibel quantisation produces exact binary values"
)]
use super::*;
use crate::{Kind, mailbox::Mailbox, tests::binding};

fn input(value: u8) -> Input {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, value]);
    mailbox.take(0, Kind::Cc, 7).unwrap()
}

#[test]
fn absolute_matches_editor_range_and_half_db_taper() {
    assert_eq!((absolute(0), absolute(127)), (-60.0, 12.0));
    assert_eq!(absolute(255), 12.0);
    assert_eq!(absolute(106), 0.0);
    assert_eq!(absolute(64), -23.5);
}

#[test]
fn pickup_waits_for_crossing_and_disarms_after_external_changes() {
    let mut pickup = Pickup::default();
    let binding = binding();
    assert_eq!(pickup.map(&binding, input(0), 0.0), None);
    assert_eq!(pickup.map(&binding, input(50), 0.0), None);
    assert_eq!(pickup.map(&binding, input(120), 0.0), Some(8.0));
    assert_eq!(pickup.map(&binding, input(121), 8.0), Some(8.5));
    assert_eq!(pickup.map(&binding, input(122), -20.0), None);
    assert_eq!(pickup.map(&binding, input(70), -20.0), Some(-20.5));
    assert_eq!(pickup.map(&binding, input(70), f32::NAN), None);
    let mut reconnected = Pickup::default();
    assert_eq!(reconnected.map(&binding, input(0), 8.5), None);
}

#[test]
fn relative_modes_obey_each_step_and_clamp_to_gain_range() {
    for step in [0.5, 1.0, 2.0, 3.0] {
        let mut binding = binding();
        binding.step_db = step;
        binding.mode = Mode::TwosComplement;
        let mut pickup = Pickup::default();
        assert_eq!(pickup.map(&binding, input(1), 0.0), Some(step));
        assert_eq!(pickup.map(&binding, input(127), 0.0), Some(-step));
        assert_eq!(pickup.map(&binding, input(63), 11.5), Some(12.0));
        assert_eq!(pickup.map(&binding, input(65), -59.0), Some(-60.0));
        binding.mode = Mode::Offset64;
        assert_eq!(pickup.map(&binding, input(65), 0.0), Some(step));
        assert_eq!(pickup.map(&binding, input(63), 0.0), Some(-step));
        assert_eq!(pickup.map(&binding, input(64), 0.0), Some(0.0));
    }
}

#[test]
fn coalescing_is_per_target_latest_wins_and_rate_has_a_hard_bound() {
    let mut queue = Coalescer::default();
    let a = binding().target;
    let b = Target {
        graph_id: "other".into(),
        ..a.clone()
    };
    assert_eq!(queue.take(&a, 0), None);
    assert_eq!(queue.desired(&a), None);
    queue.push(a.clone(), -10.0);
    queue.push(a.clone(), -5.0);
    queue.push(b.clone(), 1.0);
    assert_eq!(queue.desired(&a), Some(-5.0));
    assert_eq!(queue.take(&a, 0), Some(-5.0));
    assert_eq!(queue.desired(&a), None);
    assert_eq!(queue.take(&a, 0), None);
    assert_eq!(queue.take(&b, 0), Some(1.0));
    let mut count = 1;
    for now in 1..1000 {
        queue.push(a.clone(), 0.0);
        if queue.take(&a, now).is_some() {
            count += 1;
        }
    }
    assert_eq!(count, 30);
    queue.push(a.clone(), 10.0);
    queue.clear();
    assert_eq!(queue.take(&a, 2000), None);
}

#[test]
fn a_slow_control_acknowledgement_cannot_create_a_burst_of_trailing_revisions() {
    let mut queue = Coalescer::default();
    let target = binding().target;
    queue.complete(&target, 0);
    queue.push(target.clone(), 1.0);
    assert_eq!(queue.take(&target, 0), Some(1.0));
    queue.complete(&target, 1000);
    queue.push(target.clone(), 2.0);
    assert_eq!(queue.take(&target, 1001), None);
    assert_eq!(queue.take(&target, 1033), None);
    assert_eq!(queue.take(&target, 1034), Some(2.0));
}

#[test]
fn relative_reversals_are_coalesced_with_each_step_clamped_in_order() {
    let mut binding = binding();
    binding.mode = Mode::TwosComplement;
    binding.step_db = 3.0;
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 63]);
    mailbox.receive(&[0xb0, 7, 127]);
    let input = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(Pickup::default().map(&binding, input, 0.0), Some(9.0));
    binding.step_db = f32::NAN;
    for mode in [Mode::TwosComplement, Mode::Offset64] {
        binding.mode = mode;
        assert_eq!(Pickup::default().map(&binding, input, 0.0), None);
    }
}

#[test]
fn absolute_range_can_be_limited_or_inverted_and_pickup_uses_each_mapping_range() {
    let mut binding = crate::tests::binding();
    binding.min_db = -12.0;
    binding.max_db = 6.0;
    let mailbox = crate::mailbox::Mailbox::default();
    let mut pickup = Pickup::default();
    mailbox.receive(&[0xb0, 7, 0]);
    assert_eq!(
        pickup.map(
            &binding,
            mailbox.take(0, crate::Kind::Cc, 7).unwrap(),
            -12.0
        ),
        Some(-12.0)
    );
    mailbox.receive(&[0xb0, 7, 127]);
    assert_eq!(
        pickup.map(
            &binding,
            mailbox.take(0, crate::Kind::Cc, 7).unwrap(),
            -12.0
        ),
        Some(6.0)
    );
    binding.min_db = 6.0;
    binding.max_db = -12.0;
    let mut pickup = Pickup::default();
    mailbox.receive(&[0xb0, 7, 0]);
    assert_eq!(
        pickup.map(&binding, mailbox.take(0, crate::Kind::Cc, 7).unwrap(), 6.0),
        Some(6.0)
    );
    mailbox.receive(&[0xb0, 7, 127]);
    assert_eq!(
        pickup.map(&binding, mailbox.take(0, crate::Kind::Cc, 7).unwrap(), 6.0),
        Some(-12.0)
    );
}
