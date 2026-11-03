//! Asynchronous waits on an explicitly supplied monotonic clock.
use mixrelay_time::{Deadline, MonotonicClock};
use std::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

/// A nonblocking wait paired with the clock that produced its deadlines.
pub trait AsyncDelay: MonotonicClock {
    /// Yields until the deadline has passed.
    fn wait_until(&self, deadline: Deadline) -> impl Future<Output = ()> + Send;
}

/// Returns the operation result only while its observation deadline remains open.
pub async fn before<T>(
    timer: &impl AsyncDelay,
    deadline: Deadline,
    operation: impl Future<Output = T>,
) -> Option<T> {
    let mut operation = pin!(operation);
    let mut waiting = pin!(timer.wait_until(deadline));
    poll_fn(|context| {
        if let Poll::Ready(value) = operation.as_mut().poll(context) {
            return Poll::Ready(Some(value));
        }
        waiting.as_mut().poll(context).map(|()| None)
    })
    .await
}
