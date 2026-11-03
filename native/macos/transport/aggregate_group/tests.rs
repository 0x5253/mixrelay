use super::{AggregateGeometry, Binding, MemberInfo, Readback};

fn member(device: u32, input: u32, output: u32) -> MemberInfo {
    MemberInfo {
        device,
        input,
        output,
        rate: 48_000.0,
        alive: 1,
        ..MemberInfo::default()
    }
}

fn geometry() -> AggregateGeometry {
    AggregateGeometry::new(&[member(10, 1, 2), member(20, 3, 0), member(30, 0, 8)], 1)
        .expect("bounded geometry")
}

fn readback() -> Readback {
    let mut facts = Readback {
        primary_uid_matches: 1,
        input: 4,
        output: 10,
        ..Readback::default()
    };
    facts.active[..3].copy_from_slice(&[30, 10, 20]);
    facts.drift[..3].copy_from_slice(&[1, 0, 1]);
    facts.ordered_uids_match[..3].fill(1);
    facts.composition_uids_match[..3].fill(1);
    facts
}

#[test]
fn geometry_rejects_invalid_bounds_and_overflow_before_publication() {
    assert!(AggregateGeometry::new(&[], 0).is_none());
    assert!(AggregateGeometry::new(&[member(1, 0, 1)], 0).is_none());
    assert!(AggregateGeometry::new(&[member(1, 0, 1); 33], 0).is_none());
    assert!(AggregateGeometry::new(&[member(1, 0, 1); 2], 2).is_none());
    assert!(AggregateGeometry::new(&[member(1, u32::MAX, 0), member(2, 1, 0)], 0).is_none());
    assert!(AggregateGeometry::new(&[member(1, 0, u32::MAX), member(2, 0, 1)], 0).is_none());
    assert!(AggregateGeometry::new(&[member(1, 64, 64); 32], 31).is_some());
}

#[test]
fn freshly_resolved_member_must_keep_identity_rate_and_both_channel_counts() {
    let state = geometry();
    let original = member(10, 1, 2);
    assert!(state.member_matches(0, &original));
    for observed in [
        MemberInfo {
            device: 0,
            ..original
        },
        MemberInfo {
            device: 11,
            ..original
        },
        MemberInfo {
            alive: 0,
            ..original
        },
        MemberInfo {
            rate: 44_100.0,
            ..original
        },
        MemberInfo {
            rate: f64::NAN,
            ..original
        },
        MemberInfo {
            input: 2,
            ..original
        },
        MemberInfo {
            output: 1,
            ..original
        },
    ] {
        assert!(!state.member_matches(0, &observed));
    }
    assert!(!state.member_matches(3, &original));
}

#[test]
fn readback_requires_ordered_uid_identity_and_exact_primary() {
    let state = geometry();
    assert!(state.ready(&readback()));
    let mut facts = readback();
    facts.primary_uid_matches = 0;
    assert!(!state.ready(&facts));
    for index in 0..3 {
        let mut facts = readback();
        facts.ordered_uids_match[index] = 0;
        assert!(!state.ready(&facts));
        let mut facts = readback();
        facts.composition_uids_match[index] = 0;
        assert!(!state.ready(&facts));
    }
}

#[test]
fn active_devices_are_a_set_but_missing_duplicate_or_replaced_devices_fail() {
    let state = geometry();
    for active in [[10, 20, 30], [30, 10, 20], [20, 30, 10]] {
        let mut facts = readback();
        facts.active[..3].copy_from_slice(&active);
        assert!(state.ready(&facts));
    }
    for active in [[10, 20, 0], [10, 20, 20], [10, 20, 40]] {
        let mut facts = readback();
        facts.active[..3].copy_from_slice(&active);
        assert!(!state.ready(&facts));
    }
}

#[test]
fn correction_is_required_only_for_secondaries_and_geometry_must_be_exact() {
    let state = geometry();
    for index in 0..3 {
        let mut facts = readback();
        facts.drift[index] = i32::from(index == 1);
        assert!(!state.ready(&facts));
    }
    for (input, output) in [(3, 10), (5, 10), (4, 9), (4, 11)] {
        let facts = Readback {
            input,
            output,
            ..readback()
        };
        assert!(!state.ready(&facts));
    }
}

#[test]
fn binding_preserves_asymmetric_offsets_and_checks_requested_width() {
    let state = geometry();
    assert_eq!(
        state.binding(0, true, 1),
        Some(Binding {
            offset: 0,
            total: 4
        })
    );
    assert_eq!(
        state.binding(1, true, 3),
        Some(Binding {
            offset: 1,
            total: 4
        })
    );
    assert_eq!(
        state.binding(2, false, 8),
        Some(Binding {
            offset: 2,
            total: 10
        })
    );
    assert_eq!(
        state.binding(2, false, 1),
        Some(Binding {
            offset: 2,
            total: 10
        })
    );
    for (index, input, channels) in [
        (0, true, 0),
        (0, true, 2),
        (1, false, 1),
        (2, true, 1),
        (3, true, 1),
    ] {
        assert_eq!(state.binding(index, input, channels), None);
    }
}
