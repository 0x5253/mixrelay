//! Reservation failures injected per thread, so preparation can be tested without
//! exhausting host memory.
std::thread_local! {
    static RESERVATIONS_BEFORE_FAILURE: std::cell::Cell<Option<usize>> = const {
        std::cell::Cell::new(None)
    };
}

pub(super) fn reservation_should_fail() -> bool {
    RESERVATIONS_BEFORE_FAILURE.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    })
}

/// Lets `count` non-empty reservations succeed inside `work`, then fails the next one.
pub(in crate::render) fn fail_after<T>(count: usize, work: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RESERVATIONS_BEFORE_FAILURE.with(|remaining| remaining.set(None));
        }
    }
    RESERVATIONS_BEFORE_FAILURE.with(|remaining| {
        assert!(remaining.replace(Some(count)).is_none());
    });
    let _reset = Reset;
    work()
}
