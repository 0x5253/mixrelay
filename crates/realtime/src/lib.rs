//! Realtime scheduling and wakeups for audio threads this product owns.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    Cycle, Joined, KernReturn, Notifier, RealtimeThread, SharedFutex, ThreadScheduler,
    WorkInterval, demote_current_thread, host_time, promote_current_thread, shared_futex,
    wait_until,
};

mod scheduling;
pub use scheduling::{
    LatencyHistogram, LatencySummary, RealtimePolicy, ScheduledThread, Scheduling, Timebase,
    Transition,
};

mod notification;
pub use notification::{FutexWake, Wake};
