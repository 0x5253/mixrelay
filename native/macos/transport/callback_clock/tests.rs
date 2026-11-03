use super::*;
use std::sync::atomic::AtomicBool;

fn record(clock: &mut CallbackClock, sample: f64, host: u64, flags: bool, frames: u64) {
    // SAFETY: exclusive stack owner, initialized by Default, without concurrent readers.
    unsafe { mr_callback_clock_record_values(clock, sample, host, flags, frames) };
}

fn snapshot(clock: &CallbackClock) -> CallbackClockSnapshot {
    let mut result = CallbackClockSnapshot::default();
    // SAFETY: live initialized clock and disjoint exclusive stack output.
    assert!(unsafe { mr_callback_clock_snapshot(clock, &raw mut result) });
    result
}

#[test]
fn native_storage_and_snapshot_layout_match_both_c_callers() {
    assert_eq!(std::mem::offset_of!(CallbackClock, publication), 24);
    assert_eq!(std::mem::offset_of!(CallbackClockSnapshot, host_time), 8);
    assert_eq!(
        std::mem::offset_of!(CallbackClockSnapshot, discontinuities),
        16
    );
    assert_eq!(std::mem::offset_of!(CallbackClockSnapshot, valid), 24);
    let mut clock = std::mem::MaybeUninit::<CallbackClock>::uninit();
    // SAFETY: aligned exclusive full clock storage, not yet initialized or published.
    unsafe { mr_callback_clock_init(clock.as_mut_ptr()) };
    // SAFETY: init wrote every Rust field and established the clock value.
    let clock = unsafe { clock.assume_init() };
    assert_eq!(snapshot(&clock), CallbackClockSnapshot::default());
}

#[test]
fn invalid_timestamp_classes_unprime_once_and_allow_a_new_timeline() {
    let limit = 4_503_599_627_370_496.0;
    for (sample, flags, frames) in [
        (f64::NAN, true, 128),
        (f64::INFINITY, true, 128),
        (f64::NEG_INFINITY, true, 128),
        (limit, true, 128),
        (-limit, true, 128),
        (0.0, false, 128),
        (0.0, true, 0),
    ] {
        let mut clock = CallbackClock::default();
        record(&mut clock, -128.0, 1, true, 128);
        record(&mut clock, sample, 2, flags, frames);
        record(&mut clock, sample, 3, flags, frames);
        assert_eq!(
            snapshot(&clock),
            CallbackClockSnapshot {
                discontinuities: 1,
                ..CallbackClockSnapshot::default()
            }
        );
        record(&mut clock, 1000.0, 1, true, 128);
        assert_eq!(snapshot(&clock).valid, 1);
        assert_eq!(snapshot(&clock).discontinuities, 1);
    }
    for sample in [limit - 0.5, -limit + 0.5] {
        let mut clock = CallbackClock::default();
        record(&mut clock, sample, 1, true, u64::MAX);
        assert_eq!(snapshot(&clock).sample_time.to_bits(), sample.to_bits());
        assert_eq!(snapshot(&clock).valid, 1);
    }
}

#[test]
fn continuity_allows_half_frame_and_host_jitter_but_not_gaps_or_equal_hosts() {
    let mut clock = CallbackClock::default();
    record(&mut clock, -128.0, 100, true, 128);
    record(&mut clock, 0.5, 200, true, 128);
    record(&mut clock, 128.0, 999_999, true, 128);
    assert_eq!(snapshot(&clock).discontinuities, 0);
    record(&mut clock, 256.5001, 1_000_000, true, 128);
    assert_eq!(snapshot(&clock).discontinuities, 1);
    record(&mut clock, 384.5001, 1_000_000, true, 128);
    assert_eq!(snapshot(&clock).discontinuities, 2);
    record(&mut clock, 512.5001, 5, true, 128);
    assert_eq!(snapshot(&clock).discontinuities, 3);
}

#[test]
fn reconnect_reset_preserves_publication_and_cumulative_errors() {
    let mut clock = CallbackClock::default();
    record(&mut clock, 0.0, 100, true, 128);
    record(&mut clock, 256.0, 200, true, 128);
    let previous = snapshot(&clock);
    // SAFETY: exclusive owner models unregister/drain before reconnect.
    unsafe { mr_callback_clock_reset(&raw mut clock) };
    assert_eq!(snapshot(&clock), previous);
    record(&mut clock, -512.0, 1, true, 256);
    assert_eq!(snapshot(&clock).discontinuities, 1);
    assert_eq!(
        snapshot(&clock).sample_time.to_bits(),
        (-512.0_f64).to_bits()
    );
}

#[test]
fn stalled_publication_does_not_spin_overwrite_output_or_hide_error_count() {
    let clock = CallbackClock::default();
    clock.publication.sequence.store(1, SeqCst);
    clock.publication.discontinuities.store(7, SeqCst);
    let unchanged = CallbackClockSnapshot {
        sample_time: -10.0,
        host_time: 5,
        discontinuities: 9,
        valid: 1,
    };
    let mut result = unchanged;
    // SAFETY: initialized clock with an intentionally in-flight publication;
    // reader only accesses atomics and disjoint writable output.
    unsafe {
        assert!(!mr_callback_clock_snapshot(
            &raw const clock,
            &raw mut result
        ));
        assert_eq!(mr_callback_clock_discontinuities(&raw const clock), 7);
        assert!(!mr_callback_clock_snapshot(
            &raw const clock,
            std::ptr::null_mut()
        ));
        assert!(!mr_callback_clock_snapshot(
            std::ptr::null(),
            &raw mut result
        ));
        assert_eq!(mr_callback_clock_discontinuities(std::ptr::null()), 0);
        mr_callback_clock_init(std::ptr::null_mut());
        mr_callback_clock_reset(std::ptr::null_mut());
        mr_callback_clock_record_values(std::ptr::null_mut(), 0.0, 0, false, 0);
    }
    assert_eq!(result, unchanged);
}

#[test]
fn integer_rollover_preserves_native_wrapping_semantics_without_ffi_panics() {
    let mut clock = CallbackClock::default();
    record(&mut clock, 0.0, 100, true, 128);
    clock.publication.sequence.store(u64::MAX - 1, SeqCst);
    clock.publication.discontinuities.store(u64::MAX, SeqCst);
    record(&mut clock, 256.0, 200, true, 128);
    assert_eq!(clock.publication.sequence.load(SeqCst), 0);
    assert_eq!(snapshot(&clock).discontinuities, 0);
}

#[test]
fn readers_observe_coherent_atomic_publications_without_sharing_writer_state() {
    let publication = Publication::default();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut writer = Continuity::default();
            for index in 1..=100_000_u32 {
                let sample = f64::from(index);
                let host = u64::from(index) * 7;
                let (valid, broken) = writer.record(sample, host, true, 1);
                publication.publish(sample, host, valid, broken);
            }
            done.store(true, SeqCst);
        });
        while !done.load(SeqCst) {
            if let Some(observed) = publication.snapshot().filter(|value| value.valid != 0) {
                assert_eq!(
                    (observed.sample_time * 7.0).to_bits(),
                    f64::from(u32::try_from(observed.host_time).unwrap()).to_bits()
                );
                assert_eq!(observed.discontinuities, 0);
            }
        }
    });
    assert_eq!(publication.snapshot().unwrap().host_time, 700_000);
}

#[test]
fn native_callback_entry_and_snapshot_do_not_allocate_or_free() {
    let mut clock = CallbackClock::default();
    let measured = allocation_counter::measure(|| {
        for index in 0..10_000_u32 {
            record(
                &mut clock,
                f64::from(index) * 128.0,
                u64::from(index) + 1,
                true,
                128,
            );
            std::hint::black_box(snapshot(&clock));
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(measured.count_current, 0);
    assert_eq!(measured.count_max, 0);
    assert_eq!(snapshot(&clock).discontinuities, 0);
}
