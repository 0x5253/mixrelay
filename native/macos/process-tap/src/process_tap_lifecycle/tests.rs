use super::*;

#[test]
fn admission_is_capped_and_retirement_does_not_release_before_cleanup() {
    let owned = AtomicU32::new(0);
    let gates: [TapLifecycle; 32] = std::array::from_fn(|_| TapLifecycle::default());
    for gate in &gates {
        assert!(gate.acquire(&owned));
        assert!(gate.acquire(&owned));
    }
    let blocked = TapLifecycle::default();
    assert!(!blocked.acquire(&owned));
    assert_eq!(owned.load(Ordering::Relaxed), 32);
    assert!(gates[0].retire());
    assert!(!gates[0].retire());
    assert!(!gates[0].acquire(&owned));
    assert!(!blocked.acquire(&owned));
    gates[0].release(&owned);
    gates[0].release(&owned);
    assert_eq!(owned.load(Ordering::Relaxed), 31);
    assert!(blocked.acquire(&owned));
    blocked.release(&owned);
    for gate in &gates {
        gate.release(&owned);
    }
    assert_eq!(owned.load(Ordering::Relaxed), 0);
}

#[test]
fn stale_discovery_and_failed_restart_stay_invalid_until_fresh_preparation() {
    let gate = TapLifecycle::default();
    assert!(gate.current(0));
    let observed = gate.epoch.load(Ordering::Acquire);
    gate.changed();
    assert!(!gate.current(observed));
    assert_eq!(gate.invalidate(observed), observed);
    assert!(!gate.commit(observed));
    assert!(!gate.current(observed));
    assert!(gate.commit(gate.invalidate(1)));
    gate.changed();
    assert!(!gate.current(1));
    assert!(gate.commit(gate.invalidate(2)));
    assert!(gate.current(2));
}

#[test]
fn missed_listener_invalidates_before_cleanup_and_commit_rechecks_notifications() {
    let gate = TapLifecycle::default();
    let candidate = gate.invalidate(0);
    assert_eq!(candidate, 1);
    assert!(!gate.current(0));
    assert!(!gate.current(candidate));
    // A failed unregister leaves this gate and admission owned; no commit occurs.
    assert_eq!(gate.invalidate(candidate), candidate);
    gate.changed();
    assert!(!gate.commit(candidate));
    assert!(gate.commit(gate.invalidate(2)));
    assert!(gate.current(2));
    assert!(gate.retire());
    assert!(!gate.commit(2));
    assert!(!gate.current(2));
}

#[test]
fn epoch_exhaustion_never_wraps_back_to_a_validated_generation() {
    let gate = TapLifecycle {
        epoch: AtomicU64::new(u64::MAX - 1),
        validated: AtomicU64::new(u64::MAX - 1),
        ..TapLifecycle::default()
    };
    assert!(gate.current(u64::MAX - 1));
    gate.changed();
    gate.changed();
    assert_eq!(gate.epoch.load(Ordering::Acquire), u64::MAX);
    assert!(!gate.current(u64::MAX));
    assert_eq!(gate.invalidate(u64::MAX), u64::MAX);
    assert!(!gate.commit(u64::MAX));
    assert!(!gate.current(0));
}

#[test]
fn concurrent_admissions_hold_at_most_the_bound_until_their_owners_release() {
    let owned = AtomicU32::new(0);
    let ready = std::sync::Barrier::new(65);
    let release = std::sync::Barrier::new(65);
    std::thread::scope(|scope| {
        for _ in 0..64 {
            scope.spawn(|| {
                let gate = TapLifecycle::default();
                let acquired = gate.acquire(&owned);
                ready.wait();
                release.wait();
                if acquired {
                    gate.release(&owned);
                }
            });
        }
        ready.wait();
        assert_eq!(owned.load(Ordering::Relaxed), 32);
        release.wait();
    });
    assert_eq!(owned.load(Ordering::Relaxed), 0);
}

#[test]
fn gate_operations_allocate_nothing() {
    let gate = TapLifecycle::default();
    let measured = allocation_counter::measure(|| {
        assert!(gate.delivery_allowed());
        assert_eq!(gate.epoch(), 0);
        gate.changed();
        assert!(!gate.delivery_allowed());
        assert_eq!(gate.invalidate(1), 1);
        assert!(gate.commit(1));
        assert!(gate.current(1));
        assert!(gate.retire());
        assert!(gate.retiring());
    });
    assert_eq!(measured.count_total, 0);
}
