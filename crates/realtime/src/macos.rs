//! Realtime scheduling and wakeups for PCM threads this product owns: the fallback graph
//! worker, the application-capture pump and plug-in DSP workers. `CoreAudio` I/O procs and
//! HAL callbacks keep the scheduling `CoreAudio` gives them; supervisors, preparation,
//! XPC and reclamation never become realtime.
use crate::scheduling::{
    ScheduledThread, Scheduling, TimeConstraintPolicy, applied_policy, policy_request,
};
use crate::{FutexWake, RealtimePolicy, Timebase, Wake};
use std::{
    ffi::{CStr, c_char, c_void},
    io,
    marker::PhantomData,
    sync::atomic::AtomicU64,
    time::Duration,
};

pub type KernReturn = i32;
type MachPort = u32;

const KERN_SUCCESS: KernReturn = 0;
const KERN_OPERATION_TIMED_OUT: KernReturn = 49;
const KERN_ABORTED: KernReturn = 14;
const THREAD_STANDARD_POLICY: u32 = 1;
const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
const THREAD_TIME_CONSTRAINT_POLICY_COUNT: u32 = 4;
const SYNC_POLICY_FIFO: i32 = 0;
const OS_CLOCK_MACH_ABSOLUTE_TIME: u32 = 32;

#[repr(C)]
#[derive(Default)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[repr(C)]
struct MachTimespec {
    seconds: u32,
    nanoseconds: i32,
}

/// `os_workgroup_join_token_s`: opaque and caller-owned; 40 bytes with 4-byte alignment
/// on supported SDKs, including its signature.
#[repr(C, align(8))]
struct JoinToken([u8; 40]);

unsafe extern "C" {
    static mach_task_self_: MachPort;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> KernReturn;
    fn mach_absolute_time() -> u64;
    fn mach_wait_until(deadline: u64) -> KernReturn;
    fn mach_thread_self() -> MachPort;
    fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
    fn thread_policy_set(thread: MachPort, flavor: u32, info: *mut i32, count: u32) -> KernReturn;
    fn thread_policy_get(
        thread: MachPort,
        flavor: u32,
        info: *mut i32,
        count: *mut u32,
        get_default: *mut i32,
    ) -> KernReturn;
    fn semaphore_create(
        task: MachPort,
        semaphore: *mut MachPort,
        policy: i32,
        value: i32,
    ) -> KernReturn;
    fn semaphore_destroy(task: MachPort, semaphore: MachPort) -> KernReturn;
    fn semaphore_signal(semaphore: MachPort) -> KernReturn;
    fn semaphore_timedwait(semaphore: MachPort, wait: MachTimespec) -> KernReturn;
    fn os_workgroup_join(workgroup: *mut c_void, token: *mut JoinToken) -> i32;
    fn os_workgroup_leave(workgroup: *mut c_void, token: *mut JoinToken);
    fn os_workgroup_interval_start(
        workgroup: *mut c_void,
        start: u64,
        deadline: u64,
        data: *mut c_void,
    ) -> i32;
    fn os_workgroup_interval_finish(workgroup: *mut c_void, data: *mut c_void) -> i32;
    fn os_release(object: *mut c_void);
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioWorkIntervalCreate(name: *const c_char, clock: u32, attr: *mut c_void) -> *mut c_void;
}

/// Current host time in `mach_absolute_time` ticks.
#[must_use]
pub fn host_time() -> u64 {
    // SAFETY: a pure read of the host clock.
    unsafe { mach_absolute_time() }
}

/// Sleeps until an absolute host time, so a periodic thread keeps its phase instead of
/// accumulating each wake's lateness as a relative sleep would.
pub fn wait_until(deadline: u64) {
    // An interrupted wait returns early; wait again for the same absolute deadline.
    while host_time() < deadline {
        // SAFETY: blocks only the calling thread.
        let _ = unsafe { mach_wait_until(deadline) };
    }
}

impl Timebase {
    #[must_use]
    pub fn current() -> Self {
        let mut info = MachTimebaseInfo::default();
        // SAFETY: `info` is a valid, writable timebase structure. The call cannot fail on
        // macOS; the floor of one only keeps a zeroed result from dividing by zero.
        unsafe { mach_timebase_info(&raw mut info) };
        Self {
            numer: u64::from(info.numer.max(1)),
            denom: u64::from(info.denom.max(1)),
        }
    }
}

/// Places the calling thread in the time-constraint (realtime) class for one release
/// every `period`: `constraint` is the window it must finish within, and `computation`
/// its CPU budget inside that window.
///
/// # Errors
/// Returns invalid input for an empty period or a budget outside the window, or the
/// kernel's refusal. A rejected request leaves scheduling unchanged; if the request was
/// applied but could not be read back, the realtime policy stays in effect.
pub fn promote_current_thread(
    period: Duration,
    computation: Duration,
    constraint: Duration,
) -> io::Result<RealtimePolicy> {
    let timebase = Timebase::current();
    let mut policy = policy_request(timebase, period, computation, constraint)?;
    let thread = ThreadPort::current();
    // SAFETY: the policy is a correctly sized, writable time-constraint structure for
    // this thread's own port.
    let set = unsafe {
        thread_policy_set(
            thread.0,
            THREAD_TIME_CONSTRAINT_POLICY,
            (&raw mut policy).cast(),
            THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    // The kernel may raise the computation budget; report what it applied.
    let mut applied = TimeConstraintPolicy::default();
    let mut count = THREAD_TIME_CONSTRAINT_POLICY_COUNT;
    let mut get_default = 0;
    // SAFETY: valid output storage and count for the same policy flavor.
    let get = unsafe {
        thread_policy_get(
            thread.0,
            THREAD_TIME_CONSTRAINT_POLICY,
            (&raw mut applied).cast(),
            &raw mut count,
            &raw mut get_default,
        )
    };
    applied_policy(timebase, (set, get), get_default != 0, applied)
}

/// Returns the calling thread to standard (timeshare) scheduling.
///
/// # Errors
/// Returns the kernel's refusal.
pub fn demote_current_thread() -> io::Result<()> {
    let thread = ThreadPort::current();
    // SAFETY: the standard policy takes no policy data; this is the thread's own port.
    kern(unsafe { thread_policy_set(thread.0, THREAD_STANDARD_POLICY, std::ptr::null_mut(), 0) })
}

/// Native scheduler for the calling macOS audio thread.
#[derive(Debug, Clone, Copy)]
pub struct ThreadScheduler;

impl Scheduling for ThreadScheduler {
    fn promote(
        &self,
        period: Duration,
        computation: Duration,
        constraint: Duration,
    ) -> io::Result<RealtimePolicy> {
        promote_current_thread(period, computation, constraint)
    }
    fn demote(&self) -> io::Result<()> {
        demote_current_thread()
    }
}

pub type RealtimeThread = ScheduledThread<ThreadScheduler>;

impl ScheduledThread<ThreadScheduler> {
    /// Promotes the calling thread with the native scheduler.
    /// # Errors
    /// Returns a failed scheduling request after attempting to restore standard priority.
    pub fn promote(
        period: Duration,
        computation: Duration,
        constraint: Duration,
    ) -> io::Result<Self> {
        Self::with_scheduler(ThreadScheduler, period, computation, constraint)
    }
}

/// The thread port `mach_thread_self` returns carries a reference this guard releases.
struct ThreadPort(MachPort);

impl ThreadPort {
    fn current() -> Self {
        // SAFETY: returns the calling thread's port with one added user reference.
        Self(unsafe { mach_thread_self() })
    }
}

impl Drop for ThreadPort {
    fn drop(&mut self) {
        // SAFETY: releases exactly the reference `mach_thread_self` added.
        unsafe { mach_port_deallocate(mach_task_self_, self.0) };
    }
}

/// A work interval for a realtime thread whose deadlines are its own rather than a
/// device's IO cycle. Joining it lets the scheduler place the thread with audio work.
pub struct WorkInterval(*mut c_void);

// SAFETY: the workgroup object is a reference-counted OS object usable from any thread;
// this wrapper owns one reference.
unsafe impl Send for WorkInterval {}
// SAFETY: interval start/finish are only called by the joined thread through `Joined`.
unsafe impl Sync for WorkInterval {}

impl WorkInterval {
    /// # Errors
    /// Returns an error when `AudioToolbox` cannot create the interval.
    pub fn create(name: &CStr) -> io::Result<Self> {
        // SAFETY: a valid C string and the host clock; no attributes.
        Self::adopt(unsafe {
            AudioWorkIntervalCreate(
                name.as_ptr(),
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                std::ptr::null_mut(),
            )
        })
    }

    /// Takes ownership of one created reference, or reports that creation failed.
    fn adopt(interval: *mut c_void) -> io::Result<Self> {
        if interval.is_null() {
            return Err(io::Error::other("could not create an audio work interval"));
        }
        Ok(Self(interval))
    }

    /// Joins the calling thread until the returned guard drops on the same thread.
    ///
    /// # Errors
    /// Returns the OS error when the thread cannot join (for example, a thread already
    /// in another workgroup).
    pub fn join(&self) -> io::Result<Joined<'_>> {
        let mut token = Box::new(JoinToken([0; 40]));
        // SAFETY: a live workgroup and caller-owned token storage that outlives the
        // membership; the guard leaves with the same token.
        let result = unsafe { os_workgroup_join(self.0, &raw mut *token) };
        os(result).map(|()| Joined {
            interval: self,
            token,
            _thread: PhantomData,
        })
    }
}

impl Drop for WorkInterval {
    fn drop(&mut self) {
        // SAFETY: releases the one reference `AudioWorkIntervalCreate` returned.
        unsafe { os_release(self.0) };
    }
}

/// Membership of the joined thread. It is neither `Send` nor `Sync`: intervals are
/// started and finished, and the workgroup left, only on the thread that joined.
///
/// ```compile_fail
/// fn send<T: Send>(_: T) {}
/// let interval = mixrelay_realtime::WorkInterval::create(c"x").unwrap();
/// send(interval.join().unwrap());
/// ```
/// ```compile_fail
/// fn sync<T: Sync>(_: &T) {}
/// let interval = mixrelay_realtime::WorkInterval::create(c"x").unwrap();
/// sync(&interval.join().unwrap());
/// ```
pub struct Joined<'a> {
    interval: &'a WorkInterval,
    token: Box<JoinToken>,
    _thread: PhantomData<*const ()>,
}

impl Joined<'_> {
    /// Declares one cycle of work released at `start`, due by `deadline` (host ticks).
    /// Pass the expected release time, not the late wake time, so lateness stays visible.
    /// The cycle finishes when its guard is finished or dropped, so an early return or
    /// panic never leaves an interval running when the workgroup is left or released.
    ///
    /// # Errors
    /// Returns the OS error for an invalid interval (for example, one already started).
    pub fn start(&self, start: u64, deadline: u64) -> io::Result<Cycle<'_>> {
        // SAFETY: the calling thread joined this workgroup; no per-interval data.
        os(unsafe {
            os_workgroup_interval_start(self.interval.0, start, deadline, std::ptr::null_mut())
        })
        .map(|()| Cycle {
            joined: self,
            running: true,
        })
    }
}

/// One running interval of a joined thread.
pub struct Cycle<'a> {
    joined: &'a Joined<'a>,
    running: bool,
}

impl Cycle<'_> {
    /// # Errors
    /// Returns the OS error when the interval cannot be finished.
    pub fn finish(mut self) -> io::Result<()> {
        self.running = false;
        // SAFETY: the joined thread finishes the interval it started.
        os(unsafe { os_workgroup_interval_finish(self.joined.interval.0, std::ptr::null_mut()) })
    }
}

impl Drop for Cycle<'_> {
    fn drop(&mut self) {
        if self.running {
            // SAFETY: as in `finish`; the result is unobservable during unwinding.
            unsafe { os_workgroup_interval_finish(self.joined.interval.0, std::ptr::null_mut()) };
        }
    }
}

impl Drop for Joined<'_> {
    fn drop(&mut self) {
        // SAFETY: the joining thread leaves with the token it joined with.
        unsafe { os_workgroup_leave(self.interval.0, &raw mut *self.token) };
    }
}

/// A coalescing wakeup for one consumer thread. A notification means "recheck
/// authoritative state", never "one block is ready": producers that publish while one
/// is pending share it, so the semaphore never accumulates per-cycle tokens.
///
/// Ordering: a producer publishes its state with release stores, then calls
/// [`Notifier::notify`] (`pending` swap, acquire-release). After consuming a token the
/// consumer swaps `pending` back to zero (acquire-release) before its acquire loads of
/// state, so it observes every publication that found `pending` already set.
#[derive(Debug)]
pub struct Notifier {
    semaphore: MachPort,
    notifications: crate::notification::Notifications,
}

// SAFETY: a Mach semaphore may be signaled and waited from any thread of the task.
unsafe impl Send for Notifier {}
// SAFETY: as above; `pending` is atomic.
unsafe impl Sync for Notifier {}

impl Notifier {
    /// # Errors
    /// Returns the kernel error when no semaphore can be created.
    pub fn new() -> io::Result<Self> {
        let mut semaphore = 0;
        // SAFETY: valid output storage for this task's new semaphore.
        let created =
            unsafe { semaphore_create(mach_task_self_, &raw mut semaphore, SYNC_POLICY_FIFO, 0) };
        kern(created).map(|()| Self {
            semaphore,
            notifications: crate::notification::Notifications::default(),
        })
    }

    /// Producer side, after publishing state. Never blocks or allocates.
    pub fn notify(&self) {
        self.notifications.publish(host_time, || {
            // SAFETY: the semaphore lives as long as `self`.
            unsafe { semaphore_signal(self.semaphore) }
        });
    }

    /// Signals that could not be delivered since creation.
    #[must_use]
    pub fn failed_signals(&self) -> u64 {
        self.notifications.failed_signals()
    }

    /// Consumer side: waits for a notification or `timeout`. On a notification the
    /// pending flag is cleared before returning, so a publication racing the caller's
    /// subsequent state check either is seen by that check or posts a new token. Never
    /// allocates, including on failure.
    pub fn wait(&self, timeout: Duration) -> Wake {
        let wait = MachTimespec {
            seconds: u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX),
            nanoseconds: i32::try_from(timeout.subsec_nanos()).unwrap_or(0),
        };
        // SAFETY: the semaphore lives as long as `self`.
        let result = unsafe { semaphore_timedwait(self.semaphore, wait) };
        self.notifications
            .received(|published_at| wake_result(result, published_at))
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        // SAFETY: destroys the semaphore this notifier created; no thread can still wait
        // on it because waiting borrows `self`.
        unsafe { semaphore_destroy(mach_task_self_, self.semaphore) };
    }
}

fn wake_result(result: KernReturn, published_at: u64) -> Wake {
    match result {
        KERN_SUCCESS => Wake::Notified { published_at },
        KERN_OPERATION_TIMED_OUT | KERN_ABORTED => Wake::TimedOut,
        error => Wake::Failed(error),
    }
}

fn os(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

fn kern(result: KernReturn) -> io::Result<()> {
    if result == KERN_SUCCESS {
        Ok(())
    } else {
        Err(kern_error(result))
    }
}

fn kern_error(result: KernReturn) -> io::Error {
    io::Error::other(format!("mach kernel error {result}"))
}

/// Wait and wake on a 64-bit word in memory shared between processes, so a consumer
/// sleeps until its producer advances a sequence instead of polling it. Uses the public
/// shared-futex API, which exists from macOS 14.4; [`shared_futex`] is `None` before.
pub struct SharedFutex {
    wait: WaitOnAddress,
    wake: WakeByAddress,
}

type WaitOnAddress = unsafe extern "C" fn(*mut c_void, u64, usize, u32, u32, u64) -> i32;
type WakeByAddress = unsafe extern "C" fn(*mut c_void, usize, u32) -> i32;

const OS_SYNC_WAIT_ON_ADDRESS_SHARED: u32 = 1;
const OS_SYNC_WAKE_BY_ADDRESS_SHARED: u32 = 1;
const ETIMEDOUT: i32 = 60;
/// `RTLD_DEFAULT`: search every image loaded into the process.
const RTLD_DEFAULT: *mut c_void = -2_isize as *mut c_void;

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// The process's shared-futex API, resolved once.
#[must_use]
pub fn shared_futex() -> Option<&'static SharedFutex> {
    static FUTEX: std::sync::OnceLock<Option<SharedFutex>> = std::sync::OnceLock::new();
    FUTEX
        .get_or_init(|| {
            // SAFETY: lookups of NUL-terminated names across loaded images.
            let (wait, wake) = unsafe {
                (
                    dlsym(
                        RTLD_DEFAULT,
                        c"os_sync_wait_on_address_with_timeout".as_ptr(),
                    ),
                    dlsym(RTLD_DEFAULT, c"os_sync_wake_by_address_any".as_ptr()),
                )
            };
            SharedFutex::from_symbols(wait, wake)
        })
        .as_ref()
}

impl SharedFutex {
    /// The resolved functions, or `None` when either is absent (before macOS 14.4).
    fn from_symbols(wait: *mut c_void, wake: *mut c_void) -> Option<Self> {
        if wait.is_null() || wake.is_null() {
            return None;
        }
        // SAFETY: both symbols are the documented libSystem functions with these
        // C signatures; the non-null addresses come from the dynamic loader.
        Some(unsafe {
            Self {
                wait: std::mem::transmute::<*mut c_void, WaitOnAddress>(wait),
                wake: std::mem::transmute::<*mut c_void, WakeByAddress>(wake),
            }
        })
    }

    /// Sleeps while `word` still holds `expected`, for at most `timeout`. The caller
    /// reads `expected` before checking the state the word guards, so a producer
    /// advancing it in between makes this return at once instead of losing the wake.
    pub fn wait(&self, word: &AtomicU64, expected: u64, timeout: Duration) -> FutexWake {
        let timeout = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX).max(1);
        // SAFETY: an aligned 8-byte atomic that outlives the call; the shared flag
        // matches the producer's wake on the same mapped object.
        let result = unsafe {
            (self.wait)(
                word.as_ptr().cast(),
                expected,
                8,
                OS_SYNC_WAIT_ON_ADDRESS_SHARED,
                OS_CLOCK_MACH_ABSOLUTE_TIME,
                timeout,
            )
        };
        futex_wake(
            result,
            io::Error::last_os_error().raw_os_error().unwrap_or(0),
        )
    }

    /// Wakes one waiter after the producer advanced `word`. Never blocks; waking with no
    /// waiter is harmless.
    pub fn wake(&self, word: &AtomicU64) {
        // SAFETY: as in `wait`.
        let _ = unsafe { (self.wake)(word.as_ptr().cast(), 8, OS_SYNC_WAKE_BY_ADDRESS_SHARED) };
    }
}

fn futex_wake(result: i32, errno: i32) -> FutexWake {
    match (result, errno) {
        (0.., _) => FutexWake::Changed,
        (_, ETIMEDOUT) => FutexWake::TimedOut,
        (_, error) => FutexWake::Failed(error),
    }
}

#[cfg(test)]
#[cfg(not(debug_assertions))]
mod benchmark;
#[cfg(all(test, feature = "native-lab"))]
mod tests;
