use super::*;

#[test]
fn a_press_published_across_a_clear_boundary_is_discarded() {
    let mailbox = Mailbox::default();
    mailbox.receive_with(&[0x90, 60, 100], &mut |slot, old, next| {
        let result = slot.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire);
        // Pause after the slot CAS, immediately before the press cell and tail stores.
        mailbox.clear();
        result
    });
    assert_eq!(
        mailbox.take_presses().count(),
        0,
        "pre-clear press survived cancellation"
    );
    mailbox.receive(&[0x90, 61, 100]);
    assert_eq!(
        mailbox
            .take_presses()
            .map(|press| press.number)
            .collect::<Vec<_>>(),
        [61]
    );
}

#[test]
fn a_stale_callback_cannot_supply_volume_or_movement_to_the_next_epoch() {
    for bytes in [[0x90, 60, 100], [0xb0, 7, 127]] {
        let mailbox = Mailbox::default();
        let publication = mailbox.begin();
        mailbox.clear();
        publication.receive(&bytes);
        let kind = if bytes[0] == 0x90 {
            Kind::Note
        } else {
            Kind::Cc
        };
        assert_eq!(mailbox.take(0, kind, bytes[1]), None);
        assert_eq!(mailbox.take_presses().count(), 0);
        mailbox.receive(&[0xb0, 7, 1]);
        let input = mailbox.take(0, Kind::Cc, 7).unwrap();
        assert_eq!(input.value, 1);
        assert_eq!(input.relative_gain(false, 1.0, 0.0), Some(1.0));
    }
}

#[test]
fn clear_during_the_slot_cas_does_not_revive_old_input_or_lose_new_cc_edges() {
    let mailbox = Mailbox::default();
    mailbox.receive_with(&[0xb0, 7, 127], &mut |slot, old, next| {
        mailbox.clear();
        slot.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
    });
    assert_eq!(mailbox.take(0, Kind::Cc, 7), None);
    assert_eq!(mailbox.take_presses().count(), 0);
    mailbox.receive(&[0xb0, 7, 127]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 0);
    mailbox.receive(&[0xb0, 7, 0]);
    mailbox.receive(&[0xb0, 7, 64]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 1);
    assert_eq!(mailbox.take_presses().count(), 1);
}

#[test]
fn the_first_new_callback_replaces_old_aggregates_without_an_intervening_drain() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.receive(&[0xb0, 7, 10]);
    mailbox.clear();
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.receive(&[0xb0, 7, 2]);
    assert_eq!(mailbox.take(0, Kind::Note, 60).unwrap().presses, 1);
    let input = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(input.value, 2);
    assert_eq!(input.relative_gain(false, 1.0, 0.0), Some(2.0));
    assert_eq!(mailbox.take_presses().count(), 1);
}

#[test]
fn a_callback_inside_the_epoch_update_cannot_merge_historical_movement() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 10]);
    mailbox.clear_with(|| {
        // Pause with action publication blocked, before advancing the cancellation generation.
        mailbox.receive(&[0xb0, 7, 127]);
        mailbox.receive(&[0x90, 60, 100]);
    });
    assert_eq!(mailbox.take(0, Kind::Cc, 7), None);
    assert_eq!(mailbox.take_presses().count(), 0);
    mailbox.receive(&[0xb0, 7, 127]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 0);
    mailbox.receive(&[0xb0, 7, 2]);
    assert_eq!(
        mailbox
            .take(0, Kind::Cc, 7)
            .unwrap()
            .relative_gain(false, 1.0, 0.0),
        Some(2.0)
    );
}

#[test]
fn callback_ignores_non_channel_controls_and_invalid_data() {
    let input = Mailbox::default();
    for bytes in [
        &[][..],
        &[0xf8],
        &[0xf0, 1, 0xf7],
        &[0xb0, 128, 0],
        &[0xb0, 0, 128],
        &[0xe0, 7, 64],
    ] {
        input.receive(bytes);
    }
    assert_eq!(input.take(0, Kind::Cc, 7), None);
    assert_eq!(input.take(16, Kind::Cc, 7), None);
    assert_eq!(input.take(0, Kind::Cc, 128), None);
}

#[test]
fn notes_trigger_on_each_nonzero_note_on_and_never_on_release() {
    let input = Mailbox::default();
    for bytes in [
        [0x92, 60, 120],
        [0x92, 60, 0],
        [0x92, 60, 1],
        [0x82, 60, 90],
    ] {
        input.receive(&bytes);
    }
    let batch = input.take(2, Kind::Note, 60).unwrap();
    assert_eq!(batch.presses, 2);
    assert_eq!(batch.value, 0);
    assert_eq!(input.take(0, Kind::Note, 60), None);
    assert_eq!(input.take(2, Kind::Note, 60), None);
}

#[test]
fn cc_crosses_once_and_drain_preserves_threshold_history() {
    let input = Mailbox::default();
    for value in [63, 64, 65, 127] {
        input.receive(&[0xbf, 127, value]);
    }
    assert_eq!(input.take(15, Kind::Cc, 127).unwrap().presses, 1);
    input.receive(&[0xbf, 127, 100]);
    assert_eq!(input.take(15, Kind::Cc, 127).unwrap().presses, 0);
    for value in [0, 64] {
        input.receive(&[0xbf, 127, value]);
    }
    assert_eq!(input.take(15, Kind::Cc, 127).unwrap().presses, 1);
}

#[test]
fn relative_batches_accumulate_and_absolute_keeps_the_latest_value() {
    let input = Mailbox::default();
    for value in [1, 2, 127] {
        input.receive(&[0xb0, 7, value]);
    }
    let batch = input.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(batch.value, 127);
    assert_eq!(batch.relative_gain(false, 1.0, 0.0), Some(2.0));
    for value in [64, 65, 63] {
        input.receive(&[0xb0, 7, value]);
    }
    assert_eq!(
        input
            .take(0, Kind::Cc, 7)
            .unwrap()
            .relative_gain(true, 1.0, 0.0),
        Some(0.0)
    );
    assert_eq!(relative(0, false), 0);
    assert_eq!(relative(64, false), -64);
    assert_eq!(relative(64, true), 0);
    for _ in 0..1024 {
        input.receive(&[0xb0, 7, 63]);
    }
    assert_eq!(
        input
            .take(0, Kind::Cc, 7)
            .unwrap()
            .relative_gain(false, 1.0, 0.0),
        Some(12.0)
    );
}

#[test]
fn concurrent_drain_retries_with_current_state_and_contention_has_a_finite_budget() {
    let input = Mailbox::default();
    input.receive(&[0xb0, 7, 1]);
    let mut calls = 0;
    input.receive_with(&[0xb0, 7, 2], &mut |slot, old, next| {
        calls += 1;
        if calls == 1 {
            let batch = input.take(0, Kind::Cc, 7).unwrap();
            assert_eq!(batch.value, 1);
            Err(slot.load(Ordering::Acquire))
        } else {
            slot.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
        }
    });
    assert_eq!(calls, 2);
    assert_eq!(input.take(0, Kind::Cc, 7).unwrap().value, 2);
    calls = 0;
    input.receive_with(&[0xb0, 7, 3], &mut |slot, _, _| {
        calls += 1;
        Err(slot.load(Ordering::Acquire))
    });
    assert_eq!(calls, 8);
    assert_eq!(input.take(0, Kind::Cc, 7), None);
}

#[test]
fn learn_boundaries_discard_old_presses_without_creating_new_cc_edges() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.receive(&[0xb0, 7, 127]);
    mailbox.clear();
    assert_eq!(mailbox.take(0, Kind::Note, 60), None);
    mailbox.receive(&[0xb0, 7, 127]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 0);
}

#[test]
fn callback_and_learn_capture_allocate_nothing_under_dense_input() {
    let mailbox = Mailbox::default();
    let learn = crate::learn::LearnCapture::default();
    learn.arm();
    let counts = allocation_counter::measure(|| {
        for value in 0..=127 {
            mailbox.receive(&[0xb0, 7, value]);
            mailbox.receive(&[0x90, 60, value]);
            learn.offer(1, &[0xb0, 7, value]);
            mailbox.suppress(&[0xb0, 8, value]);
        }
    });
    assert_eq!(counts.count_total, 0);
    assert_eq!(counts.count_current, 0);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().value, 127);
}

#[test]
fn learning_tracks_cc_edges_without_replaying_the_captured_press() {
    let mailbox = Mailbox::default();
    for bytes in [&[][..], &[0x90, 60, 100], &[0xb0, 128, 1], &[0xb0, 7, 128]] {
        mailbox.suppress(bytes);
    }
    mailbox.suppress(&[0xb0, 7, 127]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7), None);
    mailbox.receive(&[0xb0, 7, 100]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 0);
    mailbox.suppress(&[0xb0, 7, 0]);
    mailbox.receive(&[0xb0, 7, 64]);
    assert_eq!(mailbox.take(0, Kind::Cc, 7).unwrap().presses, 1);
}

#[test]
fn the_latest_received_fader_wins_even_when_binding_order_is_reversed() {
    use crate::{Mode, tests::binding};
    let sequence = Arc::default();
    let first = Mailbox::new(Arc::clone(&sequence));
    let last = Mailbox::new(sequence);
    first.receive(&[0xb0, 7, 64]);
    last.receive(&[0xb0, 7, 100]);
    let a = first.take(0, Kind::Cc, 7).unwrap();
    let b = last.take(0, Kind::Cc, 7).unwrap();
    let mut binding = binding();
    for mode in [Mode::Absolute, Mode::TwosComplement, Mode::Offset64] {
        binding.mode = mode;
        assert_eq!(
            reception_order(a.sequence(&binding), b.sequence(&binding)),
            std::cmp::Ordering::Less
        );
    }
    binding.step_db = f32::NAN;
    assert_eq!(a.sequence(&binding), a.sequence);
    binding.action = crate::Action::Mute;
    assert_eq!(a.sequence(&binding), a.sequence);
    let mut reversed = [b, a];
    reversed
        .sort_by(|left, right| reception_order(left.sequence(&binding), right.sequence(&binding)));
    assert_eq!(reversed[1].value, 100);
    assert_eq!(
        reception_order(0, SEQUENCE_MASK),
        std::cmp::Ordering::Greater
    );
    assert_eq!(reception_order(1, 1), std::cmp::Ordering::Equal);
}

#[test]
fn discrete_presses_keep_receipt_order_and_clear_discards_them() {
    use crate::tests::binding;
    let sequence = Arc::default();
    let first = Mailbox::new(Arc::clone(&sequence));
    let second = Mailbox::new(sequence);
    first.receive(&[0x90, 60, 100]);
    second.receive(&[0xb2, 8, 127]);
    first.receive(&[0x80, 60, 0]);
    first.receive(&[0x90, 60, 100]);
    let mut presses: Vec<_> = second.take_presses().chain(first.take_presses()).collect();
    let mut binding = binding();
    binding.action = Action::Record;
    presses.sort_by(|a, b| reception_order(a.sequence(&binding), b.sequence(&binding)));
    assert_eq!(
        presses
            .iter()
            .map(|p| (p.channel, p.kind, p.number, p.presses))
            .collect::<Vec<_>>(),
        vec![
            (0, Kind::Note, 60, 1),
            (2, Kind::Cc, 8, 1),
            (0, Kind::Note, 60, 1)
        ]
    );
    assert_eq!(first.take_presses().count(), 0);
    first.receive(&[0x90, 60, 100]);
    first.clear();
    assert_eq!(first.take_presses().count(), 0);
}

#[test]
fn discrete_storage_stays_bounded_and_overflow_keeps_the_latest_presses() {
    let mailbox = Mailbox::default();
    for _ in 0..PRESS_CAPACITY + 10 {
        mailbox.receive(&[0x90, 60, 100]);
    }
    let mut presses: Vec<_> = mailbox.take_presses().collect();
    presses.sort_by(|a, b| reception_order(a.sequence, b.sequence));
    assert_eq!(presses.len(), PRESS_CAPACITY);
    assert_eq!(presses.first().unwrap().sequence, 10);
    assert_eq!(
        presses.last().unwrap().sequence,
        (PRESS_CAPACITY + 9) as u64
    );
}

#[test]
fn presses_published_after_a_drain_boundary_wait_for_the_next_drain_in_order() {
    let mailbox = Mailbox::default();
    let mut draining = mailbox.take_presses();
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.receive(&[0x90, 61, 100]);
    assert!(
        draining.next().is_none(),
        "an empty drain cannot acquire later presses"
    );
    drop(draining);
    assert_eq!(
        mailbox
            .take_presses()
            .map(|input| input.number)
            .collect::<Vec<_>>(),
        [60, 61]
    );
    mailbox.receive(&[0x90, 62, 100]);
    assert_eq!(
        mailbox
            .take_presses()
            .map(|input| input.number)
            .collect::<Vec<_>>(),
        [62]
    );
}

#[test]
fn an_overrun_during_a_drain_never_pulls_a_later_press_ahead_of_its_prefix() {
    for count in [PRESS_CAPACITY, 1 << 14] {
        let mailbox = Mailbox::default();
        mailbox.receive(&[0x90, 60, 100]);
        let mut draining = mailbox.take_presses();
        for _ in 0..count {
            mailbox.receive(&[0x90, 61, 100]);
        }
        assert!(draining.next().is_none());
        drop(draining);
        let presses: Vec<_> = mailbox.take_presses().collect();
        assert_eq!(presses.len(), PRESS_CAPACITY);
        assert!(presses.iter().all(|press| press.number == 61));
        assert!(
            presses
                .windows(2)
                .all(|pair| reception_order(pair[0].sequence, pair[1].sequence).is_lt())
        );
    }
}

#[test]
fn a_shared_boundary_defers_later_ports_and_exposes_in_flight_publications() {
    let sequence = Arc::default();
    let first = Mailbox::new(Arc::clone(&sequence));
    let second = Mailbox::new(sequence);
    let boundary = first.boundary();
    first.receive_with(&[0x90, 60, 100], &mut |slot, old, next| {
        assert!(first.is_publishing());
        second.receive(&[0x90, 61, 100]);
        assert!(first.take_presses_before(boundary).next().is_none());
        assert!(second.take_presses_before(boundary).next().is_none());
        slot.compare_exchange(old, next, Ordering::AcqRel, Ordering::Acquire)
    });
    assert!(!first.is_publishing());
    let boundary = first.boundary();
    let mut presses: Vec<_> = second
        .take_presses_before(boundary)
        .chain(first.take_presses_before(boundary))
        .collect();
    presses.sort_by(|a, b| reception_order(a.sequence, b.sequence));
    assert_eq!(
        presses.iter().map(|input| input.number).collect::<Vec<_>>(),
        [60, 61]
    );
}

#[test]
fn an_old_drain_leaves_post_cancellation_movement_for_the_next_drain() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 10]);
    let old = mailbox
        .take_with(0, Kind::Cc, 7, &mut |stage| {
            if stage != TakeStage::Consumed {
                return;
            }
            // The old reader has accepted its raw word, but has not consumed movement.
            mailbox.clear();
            mailbox.receive(&[0xb0, 7, 2]);
        })
        .unwrap();
    assert_eq!(old.relative_gain(false, 1.0, 0.0), Some(0.0));
    let fresh = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(fresh.value, 2);
    assert_eq!(fresh.relative_gain(false, 1.0, 0.0), Some(2.0));
}

#[test]
fn fresh_input_survives_half_a_receipt_range_without_a_clear() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 1]);
    // Pause receipt allocation at the half-range boundary; cancellation has not aged.
    mailbox.sequence.0.store(1 << 37, Ordering::Release);
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.receive(&[0x90, 60, 100]);
    assert_eq!(mailbox.take(0, Kind::Note, 60).unwrap().presses, 2);
    assert_eq!(
        mailbox.take_presses().map(|p| p.number).collect::<Vec<_>>(),
        [60, 60]
    );
    mailbox.receive(&[0xb0, 7, 2]);
    mailbox.receive(&[0xb0, 7, 3]);
    let input = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(input.relative_gain(false, 1.0, 0.0), Some(6.0));
}

#[test]
fn a_drain_defers_raw_values_replaced_after_its_generation_check() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 1]);
    assert_eq!(
        mailbox.take_with(0, Kind::Cc, 7, &mut |stage| {
            assert!(stage == TakeStage::Checked);
            mailbox.receive(&[0xb0, 7, 2]);
        }),
        None
    );
    let fresh = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(fresh.value, 2);
    assert_eq!(fresh.relative_gain(false, 1.0, 0.0), Some(3.0));
}

#[test]
fn readers_defer_partial_generation_publications_without_waiting() {
    let mailbox = Mailbox::default();
    mailbox.receive(&[0xb0, 7, 10]);
    mailbox.receive(&[0x90, 60, 100]);
    mailbox.clear();
    mailbox.receive_with(&[0xb0, 7, 2], &mut |slot, old, next| {
        // A new generation is being published; readers cannot mix it with old payload.
        assert_eq!(mailbox.take(0, Kind::Cc, 7), None);
        slot.compare_exchange(old, next, Ordering::SeqCst, Ordering::SeqCst)
    });
    let fresh = mailbox.take(0, Kind::Cc, 7).unwrap();
    assert_eq!(fresh.relative_gain(false, 1.0, 0.0), Some(2.0));
    {
        let (_writing, _) = mailbox.presses[0].publish(1);
        assert_eq!(mailbox.take_presses().count(), 0);
        assert_eq!(mailbox.consumed_press.load(Ordering::Acquire), 0);
        // Complete the pending press cell before making it visible to the reader.
        mailbox.presses[0]
            .value
            .store((1 << 12) | 0x3d, Ordering::SeqCst);
    }
    assert_eq!(
        mailbox.take_presses().map(|p| p.number).collect::<Vec<_>>(),
        [61]
    );
}
