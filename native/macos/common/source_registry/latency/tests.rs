use super::*;
fn input() -> Observation {
    Observation {
        epoch: 1,
        published: 0,
        frames: 128,
        capacity: 16384,
        producer_burst: 0,
        converter_burst: 128,
        drought: 0,
        running: true,
        adjacent: false,
        copy_missing: 0,
    }
}
#[test]
fn bootstrap_measured_bursts_rounding_and_capacity_bound() {
    let (state, ready) = transition(Latency::default(), input());
    assert!(!ready);
    assert_eq!(state.target, 640);
    let (state, _) = transition(
        state,
        Observation {
            producer_burst: 129,
            drought: 1,
            ..input()
        },
    );
    assert_eq!(state.target, 384);
    let (state, _) = transition(
        state,
        Observation {
            producer_burst: 128,
            converter_burst: 400,
            ..input()
        },
    );
    assert_eq!(state.target, 640);
    let mut state = state;
    for _ in 0..8 {
        state = transition(
            state,
            Observation {
                producer_burst: 128,
                adjacent: true,
                ..input()
            },
        )
        .0;
    }
    assert_eq!(state.consumer_burst, 1152);
    assert_eq!(state.target, 1280);
    let (state, _) = transition(
        state,
        Observation {
            producer_burst: u32::MAX,
            drought: u32::MAX,
            ..input()
        },
    );
    assert_eq!(state.target, 8192);
    assert!(state.insufficient_headroom);
}
#[test]
fn starvation_grows_once_holds_cursor_and_never_shrinks_live() {
    let (mut state, ready) = transition(
        Latency::default(),
        Observation {
            published: 1024,
            producer_burst: 128,
            ..input()
        },
    );
    assert!(ready);
    assert_eq!(state.next, 768);
    state.next = 1024;
    let (mut state, ready) = transition(
        state,
        Observation {
            published: 1024,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(state.target, 640);
    assert_eq!(state.starvation_episodes, 1);
    for _ in 0..10 {
        state = transition(
            state,
            Observation {
                published: 1024,
                producer_burst: 4096,
                ..input()
            },
        )
        .0;
    }
    assert_eq!(state.next, 1024);
    assert_eq!(state.target, 640);
    assert_eq!(state.starvation_episodes, 1);
    assert_eq!(state.starvation_frames, 11 * 128);
    assert_eq!(state.rebuffer_frames, 11 * 128);
    let (state, ready) = transition(
        state,
        Observation {
            published: 1536,
            ..input()
        },
    );
    assert!(!ready); // Enough for a block, but not the recovery target.
    assert_eq!(state.starvation_frames, 11 * 128);
    let (mut state, ready) = transition(
        state,
        Observation {
            published: 1664,
            producer_burst: 128,
            ..input()
        },
    );
    assert!(ready);
    assert!(state.rebuffering);
    assert_eq!(state.next, 1024);
    state = state.consumed(128);
    assert!(!state.rebuffering);
    assert_eq!(state.next, 1152);
    assert_eq!(state.target, 640);
    state.next = 1664;
    let (state, _) = transition(
        state,
        Observation {
            published: 1664,
            producer_burst: 128,
            ..input()
        },
    );
    assert_eq!(state.target, 960);
    assert_eq!(state.starvation_episodes, 2);
}
#[test]
fn stopped_epoch_overwrite_and_copy_races_have_distinct_outcomes() {
    let state = Latency {
        epoch: 1,
        next: 100,
        started: true,
        target: 256,
        ..Latency::default()
    };
    let (stopped, ready) = transition(
        state,
        Observation {
            published: 100,
            running: false,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(stopped.starvation_episodes, 0);
    assert_eq!(stopped.next, 100);
    let (overwritten, ready) = transition(
        state,
        Observation {
            published: 20000,
            ..input()
        },
    );
    assert!(ready);
    assert_eq!(overwritten.next, 19744);
    assert_eq!(overwritten.overwrite_skips, 19644);
    let (failed, ready) = transition(
        state,
        Observation {
            published: 1000,
            copy_missing: 128,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(failed.next, 100);
    assert_eq!(failed.starvation_episodes, 1);
    let (restarted, ready) = transition(
        failed,
        Observation {
            epoch: 2,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(restarted.next, 0);
    assert!(!restarted.rebuffering);
    assert_eq!(restarted.target, 640);
}

#[test]
fn repeated_large_deficits_are_bounded_and_policy_steps_allocate_nothing() {
    let mut state = Latency {
        epoch: 1,
        next: 5000,
        started: true,
        target: 8000,
        ..Latency::default()
    };
    let measured = allocation_counter::measure(|| {
        let (next, ready) = transition(
            state,
            Observation {
                published: 5100,
                ..input()
            },
        );
        state = next;
        assert!(!ready);
        assert_eq!(state.target, 8192);
        assert_eq!(state.starvation_frames, 28);
        for _ in 0..100 {
            state = transition(
                state,
                Observation {
                    published: 5100,
                    running: false,
                    ..input()
                },
            )
            .0;
        }
        assert_eq!(state.target, 8192);
        assert_eq!(state.starvation_episodes, 1);
        // A new epoch may reinitialize a lower target; a healthy live one may not.
        let (next, ready) = transition(
            state,
            Observation {
                epoch: 2,
                published: 1024,
                producer_burst: 128,
                ..input()
            },
        );
        assert!(ready);
        assert_eq!(next.target, 256);
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn a_racing_copy_does_not_end_rebuffer_episode_before_pcm_is_consumed() {
    let state = Latency {
        epoch: 1,
        next: 1024,
        started: true,
        target: 384,
        rebuffering: true,
        starvation_episodes: 1,
        ..Latency::default()
    };
    let (ready_state, ready) = transition(
        state,
        Observation {
            published: 1536,
            ..input()
        },
    );
    assert!(ready);
    let (failed, ready) = transition(
        ready_state,
        Observation {
            published: 1536,
            copy_missing: 64,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(failed.target, 384);
    assert_eq!(failed.starvation_episodes, 1);
    assert_eq!(failed.starvation_frames, 64);
    assert_eq!(failed.next, 1024);
    let consumed = failed.consumed(128);
    assert_eq!(consumed.next, 1152);
    assert!(!consumed.rebuffering);
}

#[test]
fn impossible_consumer_quantum_holds_cursor_and_reports_headroom() {
    let state = Latency {
        epoch: 1,
        next: 100,
        started: true,
        target: 256,
        ..Latency::default()
    };
    let (next, ready) = transition(
        state,
        Observation {
            capacity: 1024,
            frames: 4096,
            published: 2000,
            ..input()
        },
    );
    assert!(!ready);
    assert_eq!(next.next, 100);
    assert_eq!(next.target, 512);
    assert!(next.insufficient_headroom);
    assert_eq!(next.starvation_episodes, 0);
}

#[test]
fn shorter_final_slice_preserves_graph_quantum_margin() {
    let state = transition(Latency::default(), input()).0;
    let (state, _) = transition(
        state,
        Observation {
            frames: 32,
            adjacent: true,
            ..input()
        },
    );
    assert_eq!(state.consumer_burst, 160);
    assert_eq!(state.target, 640);
    assert_eq!(state.quantum, 128);
}
