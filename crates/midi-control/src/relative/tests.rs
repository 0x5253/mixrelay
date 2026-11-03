use super::*;

#[test]
fn batched_movement_preserves_reversals_after_clipping_at_either_limit() {
    let movement = AtomicMovement::new();
    movement.append(378, 1, false); // +63 ticks at 3 dB, followed by one negative tick.
    movement.append(-6, 1, false);
    assert_eq!(movement.take(1).apply(0.0).to_bits(), 9.0_f32.to_bits());
    movement.append(-384, 1, false);
    movement.append(2, 1, false);
    assert_eq!(movement.take(1).apply(0.0).to_bits(), (-59.0_f32).to_bits());
    assert_eq!(movement.take(1).apply(-3.5).to_bits(), (-3.5_f32).to_bits());
}

#[test]
fn clipped_translation_matches_sequential_controls_across_the_complete_gain_range() {
    for start in -120_i16..=24 {
        for deltas in [[1, -1, 0], [378, -6, 2], [-384, 2, -1], [10, -20, 15]] {
            let mut expected = f32::from(start) / 2.0;
            let movement = AtomicMovement::new();
            for delta in deltas {
                movement.append(delta, 1, false);
                expected = (expected + f32::from(delta) / 2.0).clamp(-60.0, 12.0);
            }
            assert_eq!(
                movement.take(1).apply(f32::from(start) / 2.0).to_bits(),
                expected.to_bits()
            );
        }
    }
}

#[test]
fn a_concurrent_drain_preserves_the_new_delta_and_retries_have_a_fixed_budget() {
    let movement = AtomicMovement::new();
    movement.append(2, 1, false);
    let mut calls = 0;
    movement.append_with(-2, 2, false, &mut |cell, old, next| {
        calls += 1;
        if calls == 1 {
            assert_eq!(movement.take(1).apply(0.0).to_bits(), 1.0_f32.to_bits());
            Err(cell.load(Ordering::Acquire))
        } else {
            cell.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
        }
    });
    assert_eq!(movement.take(2).apply(0.0).to_bits(), (-1.0_f32).to_bits());
    movement.append_with(1, 3, false, &mut |cell, _, _| {
        calls += 1;
        Err(cell.load(Ordering::Acquire))
    });
    assert_eq!(calls, 10);
}

#[test]
fn movement_consumption_rechecks_receipts_and_has_a_finite_retry_budget() {
    let movement = AtomicMovement::new();
    movement.append(2, 1, false);
    let mut calls = 0;
    let old = movement.take_with(1, &mut |cell, _, _| {
        calls += 1;
        movement.append(4, 2, true);
        Err(cell.load(Ordering::Acquire))
    });
    assert_eq!(calls, 1);
    assert_eq!(old.apply(0.0).to_bits(), 0.0_f32.to_bits());
    assert_eq!(movement.take(2).apply(0.0).to_bits(), 2.0_f32.to_bits());
    movement.append(2, 3, false);
    calls = 0;
    let deferred = movement.take_with(3, &mut |cell, _, _| {
        calls += 1;
        Err(cell.load(Ordering::Acquire))
    });
    assert_eq!(calls, 8);
    assert_eq!(deferred.apply(0.0).to_bits(), 0.0_f32.to_bits());
    assert_eq!(movement.take(3).apply(0.0).to_bits(), 1.0_f32.to_bits());
}
