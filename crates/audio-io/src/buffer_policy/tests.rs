use super::*;

fn policy(queue_rate: u32, peer_rate: u32, relation: ClockRelation) -> QueueTargetPolicy {
    QueueTargetPolicy::new(
        queue_rate,
        peer_rate,
        relation,
        QueueBounds {
            minimum_frames: 32,
            capacity_frames: 16_384,
            maximum_target_frames: 8192,
        },
    )
    .unwrap()
}

#[test]
fn callback_rate_conversion_rounds_up_before_applying_clock_reserve() {
    for (queue_rate, peer_rate, queue, peer, converter, aligned, independent) in [
        (48_000, 48_000, 0, 0, 1, 32, 64),
        (48_000, 96_000, 32, 128, 1, 64, 128),
        (96_000, 48_000, 32, 128, 1, 256, 512),
        (48_000, 44_100, 32, 128, 1, 140, 280),
        (44_100, 48_000, 32, 128, 1, 118, 236),
        (48_000, 48_000, 32, 32, 130, 130, 260),
        (96_000, 48_000, 32, 32, 260, 260, 520),
        (48_000, 48_000, 512, 32, 128, 512, 1024),
        (48_000, 48_000, 8192, 128, 128, 8192, 8192),
        (384_000, 8_000, 32, 4096, 128, 8192, 8192),
    ] {
        assert_eq!(
            policy(queue_rate, peer_rate, ClockRelation::ProvenSameClock)
                .target_frames(queue, peer, converter),
            aligned
        );
        assert_eq!(
            policy(queue_rate, peer_rate, ClockRelation::Independent)
                .target_frames(queue, peer, converter),
            independent
        );
    }
}

#[test]
fn same_nominal_rate_never_implies_one_clock() {
    let independent = policy(48_000, 48_000, ClockRelation::Independent);
    let shared = policy(48_000, 48_000, ClockRelation::ProvenSameClock);
    assert_eq!(independent.target_frames(256, 32, 1), 512);
    assert_eq!(shared.target_frames(256, 32, 1), 256);
}

#[test]
fn capacity_is_only_a_limit_not_a_target_or_channel_scaled_latency() {
    let target = policy(48_000, 96_000, ClockRelation::Independent);
    for capacity_frames in [
        1024,
        4096,
        16_384,
        65_536,
        crate::retention::RING_FRAMES as usize,
    ] {
        let resized = QueueTargetPolicy::new(
            48_000,
            96_000,
            ClockRelation::Independent,
            QueueBounds {
                minimum_frames: 32,
                capacity_frames,
                maximum_target_frames: 8192.min(capacity_frames / 2),
            },
        )
        .unwrap();
        assert_eq!(
            target.target_frames(32, 64, 128),
            resized.target_frames(32, 64, 128)
        );
    }
}

#[test]
fn extremes_saturate_without_overflow_or_division_by_zero() {
    for (queue, peer) in [(1, u32::MAX), (u32::MAX, 1), (u32::MAX, u32::MAX)] {
        for relation in [ClockRelation::Independent, ClockRelation::ProvenSameClock] {
            let target = policy(queue, peer, relation);
            assert_eq!(target.target_frames(u32::MAX, u32::MAX, usize::MAX), 8192);
            assert!((32..=8192).contains(&target.target_frames(0, u32::MAX, 1)));
        }
    }
}

#[test]
fn invalid_configuration_is_rejected_before_callback_use() {
    let bounds = QueueBounds {
        minimum_frames: 32,
        capacity_frames: 64,
        maximum_target_frames: 32,
    };
    for (queue, peer) in [(0, 0), (48_000, 0), (0, 48_000)] {
        assert_eq!(
            QueueTargetPolicy::new(queue, peer, ClockRelation::Independent, bounds),
            Err(QueuePolicyError::ZeroRate)
        );
    }
    for bounds in [
        QueueBounds {
            minimum_frames: 0,
            capacity_frames: 64,
            maximum_target_frames: 32,
        },
        QueueBounds {
            minimum_frames: 32,
            capacity_frames: 63,
            maximum_target_frames: 32,
        },
        QueueBounds {
            minimum_frames: 1,
            capacity_frames: 0,
            maximum_target_frames: 1,
        },
    ] {
        assert_eq!(
            QueueTargetPolicy::new(48_000, 48_000, ClockRelation::Independent, bounds),
            Err(QueuePolicyError::InvalidBounds)
        );
    }
}

#[test]
fn calculations_are_allocation_free_and_do_not_change_policy() {
    let target = policy(48_000, 44_100, ClockRelation::Independent);
    let allocation = allocation_counter::measure(|| {
        for quantum in 0..10_000 {
            std::hint::black_box(target.target_frames(quantum, quantum + 1, 128));
        }
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.count_current, 0);
    assert_eq!(target.target_frames(32, 128, 1), 280);
}
