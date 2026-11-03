//! Tokio waits paired with the product monotonic clock.
use mixrelay_desktop_application::timing::AsyncDelay;
use mixrelay_time::{Deadline, Instant, MonotonicClock, StdClock};

/// One timer origin is shared by every deadline in an application operation.
pub(crate) struct TokioDelay {
    clock: StdClock,
}
impl TokioDelay {
    pub(crate) fn new(clock: StdClock) -> Self {
        Self { clock }
    }
}
impl MonotonicClock for TokioDelay {
    fn now(&self) -> Instant {
        self.clock.now()
    }
}
impl AsyncDelay for TokioDelay {
    async fn wait_until(&self, deadline: Deadline) {
        tokio::time::sleep(deadline.remaining(self.clock.now())).await;
    }
}
