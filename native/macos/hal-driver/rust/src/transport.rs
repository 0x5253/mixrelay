//! Requester-specific presentation, independent of registry and audio locks.
//! A dedicated worker performs process/filesystem inspection. Scoped IO entries
//! return `virt` without identity work, including reentrant property queries.
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, mpsc};
use std::time::Duration;

use mixrelay_time::{Instant, MonotonicClock};

use crate::device::lock;

pub(crate) const VIRTUAL: u32 = u32::from_be_bytes(*b"virt");
const USB: u32 = u32::from_be_bytes(*b"usb ");
const PENDING: u32 = 0;
const NEGATIVE_TTL: Duration = Duration::from_secs(2);
pub(crate) const QUERY_BUDGET: Duration = Duration::from_millis(20);
const MAX_IDENTITIES: usize = 4096;
const RECLAIM_STEP: usize = 16;
const RECLAIM_INTERVAL: Duration = Duration::from_millis(10);

// Property reads never wait on a mutex. Contention is a transient fallback,
// including poisoned locks (whose state remains usable as on the control path).
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}
thread_local! { static AUDIO_DEPTH: Cell<u32> = const { Cell::new(0) }; }
pub(crate) fn enter_audio_thread() {
    AUDIO_DEPTH.set(AUDIO_DEPTH.get().saturating_add(1));
}
pub(crate) fn leave_audio_thread() {
    AUDIO_DEPTH.set(AUDIO_DEPTH.get().saturating_sub(1));
}
/// Whether this thread is inside an IO or timestamp callback.
pub(crate) fn on_audio_thread() -> bool {
    AUDIO_DEPTH.get() != 0
}
pub(crate) struct AudioScope;
impl AudioScope {
    pub(crate) fn enter() -> Self {
        enter_audio_thread();
        Self
    }
}
impl Drop for AudioScope {
    fn drop(&mut self) {
        leave_audio_thread();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Incarnation(pub(crate) i32, pub(crate) u64, pub(crate) u64);
struct Client {
    pid: i32,
    incarnation: Option<Incarnation>,
    bundle: Option<String>,
}
/// A process's bundle from `AddDeviceClient` and the incarnation seen then.
struct Registered {
    incarnation: Option<Incarnation>,
    bundle: String,
    /// Set by a property read that found another incarnation on the PID. It
    /// needs no lock, so contention cannot lose that finding.
    retired: Arc<AtomicBool>,
}
struct Identity {
    bundle: Option<String>,
    expires: Instant,
    answer: Arc<AtomicU32>,
    in_flight: bool,
}
#[derive(Default)]
pub(crate) struct State {
    policy: Vec<String>,
    clients: BTreeMap<(u32, u32), Client>,
    /// Each registered process's bundle, kept until the process's last client
    /// registration is removed.
    bundles: BTreeMap<i32, Registered>,
    identities: BTreeMap<Incarnation, Identity>,
}
impl State {
    fn policy(&mut self, bundles: Vec<String>) -> bool {
        if self.policy == bundles {
            return false;
        }
        self.policy = bundles;
        self.identities.clear();
        true
    }
    fn client(&mut self, device: u32, client: u32, identity: Client) {
        match &identity.bundle {
            Some(bundle) => {
                self.bundles.insert(
                    identity.pid,
                    Registered {
                        incarnation: identity.incarnation,
                        bundle: bundle.clone(),
                        retired: Arc::new(AtomicBool::new(false)),
                    },
                );
            }
            // A new process reusing the PID before its predecessor's registrations
            // are removed must not inherit that bundle. Without both incarnations
            // the registrations cannot be told apart, so the bundle stays.
            None => {
                if let (Some(new), Some(Some(known))) = (
                    identity.incarnation,
                    self.bundles.get(&identity.pid).map(|seen| seen.incarnation),
                ) && new != known
                {
                    self.bundles.remove(&identity.pid);
                }
            }
        }
        if let Some(old) = self.clients.insert((device, client), identity) {
            self.reclaim_client(old.pid);
        }
    }
    fn remove(&mut self, device: u32, client: u32) {
        if let Some(old) = self.clients.remove(&(device, client)) {
            self.reclaim_client(old.pid);
        }
    }
    fn reclaim_client(&mut self, pid: i32) {
        if !self.clients.values().any(|client| client.pid == pid) {
            self.identities.retain(|key, _| key.0 != pid);
            self.bundles.remove(&pid);
        }
    }
    /// The answer for the bundle coreaudiod named for this PID in `AddDeviceClient`,
    /// with the incarnation seen then. The registration lasts until
    /// `RemoveDeviceClient`, so it needs no process inspection.
    fn registered(&self, pid: i32) -> Option<(&Registered, u32)> {
        self.bundles
            .get(&pid)
            .filter(|seen| !seen.retired.load(Ordering::Acquire))
            .map(|seen| (seen, self.answer(&seen.bundle)))
    }
    /// Drops a registered bundle already marked as an exited incarnation's.
    fn retire_bundle(&mut self, pid: i32, retired: &Arc<AtomicBool>) {
        if self
            .bundles
            .get(&pid)
            .is_some_and(|seen| Arc::ptr_eq(&seen.retired, retired))
        {
            self.bundles.remove(&pid);
        }
    }
    fn answer(&self, bundle: &str) -> u32 {
        if self.policy.iter().any(|allowed| allowed == bundle) {
            USB
        } else {
            VIRTUAL
        }
    }
    fn known(&self, key: Incarnation) -> Option<String> {
        self.clients.values().find_map(|client| {
            (client.pid == key.0 && client.incarnation == Some(key))
                .then(|| client.bundle.clone())
                .flatten()
        })
    }
    fn resolve(&mut self, key: Incarnation, now: Instant, discovered: Option<String>) {
        // Callback identity takes precedence even if it arrived during discovery.
        let bundle = self.known(key).or(discovered);
        let Some(identity) = self.identities.get_mut(&key) else {
            return;
        };
        let value = if bundle
            .as_ref()
            .is_some_and(|bundle| self.policy.contains(bundle))
        {
            USB
        } else {
            VIRTUAL
        };
        // Only completed identity work publishes a stable decision. Queue-full
        // and caller timeouts never replace PENDING with a permanent negative.
        let _ =
            identity
                .answer
                .compare_exchange(PENDING, value, Ordering::AcqRel, Ordering::Acquire);
        identity.bundle = bundle;
        identity.expires = now.saturating_add(NEGATIVE_TTL);
        identity.in_flight = false;
    }
}

/// Time as one property read observes it: its deadline checks and its discovery wait.
trait Clock: MonotonicClock {
    /// Waits at most `timeout` for discovery to signal completion.
    fn wait(&self, completion: &mpsc::Receiver<()>, timeout: Duration);
}

struct SystemClock<'a>(&'a dyn MonotonicClock);
impl MonotonicClock for SystemClock<'_> {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl Clock for SystemClock<'_> {
    fn wait(&self, completion: &mpsc::Receiver<()>, timeout: Duration) {
        let _ = completion.recv_timeout(timeout);
    }
}

struct Query {
    key: Incarnation,
    answer: Arc<AtomicU32>,
    done: mpsc::SyncSender<()>,
}
/// How the HAL may inspect other processes. The installed driver host runs under
/// coreaudiod's sandbox profile (`deny default`, no `process-info-pidinfo`), which denies
/// `proc_pidinfo`/`proc_pidpath` for other processes, so every identity decision must also
/// hold with `SANDBOXED`; tests default to it.
#[derive(Clone, Copy)]
pub(crate) struct ProcessInspection {
    pub(crate) incarnation: fn(i32) -> Option<Incarnation>,
    pub(crate) discover: fn(Incarnation) -> Option<String>,
}
impl ProcessInspection {
    pub(crate) const SYSTEM: Self = Self {
        incarnation,
        discover,
    };
    #[cfg(test)]
    pub(crate) const SANDBOXED: Self = Self {
        incarnation: |_| None,
        discover: |_| None,
    };
    /// One process incarnation's start time packed into a nonzero word, so a reused PID never
    /// matches. `None` when the process is gone or the sandbox denies the query.
    pub(crate) fn process_start(self, pid: u32) -> Option<u64> {
        let Incarnation(_, seconds, micros) = (self.incarnation)(i32::try_from(pid).ok()?)?;
        Some((seconds << 20 | micros).max(1))
    }
}

const COREAUDIOD_BUNDLE: &str = "com.apple.audio.coreaudiod";

pub(crate) struct Transport {
    pub(crate) state: Arc<Mutex<State>>,
    queue: mpsc::SyncSender<Query>,
    worker: Option<std::thread::JoinHandle<()>>,
    inspection: ProcessInspection,
    clock: Arc<dyn MonotonicClock>,
    /// The PID coreaudiod registered under its own bundle, or 0. Property reads classify
    /// requesters from this atomic, so lock contention never changes the answer.
    coreaudio_pid: AtomicI32,
}
impl Transport {
    pub(crate) fn with_inspection(
        inspection: ProcessInspection,
        clock: impl MonotonicClock + 'static,
    ) -> Self {
        let mut transport =
            Self::with_resolvers(inspection.discover, inspection.incarnation, clock);
        transport.inspection = inspection;
        transport
    }
    #[cfg(test)]
    fn with_discovery(lookup: impl Fn(Incarnation) -> Option<String> + Send + 'static) -> Self {
        Self::with_resolvers(lookup, incarnation, mixrelay_time::StdClock::new())
    }
    fn with_resolvers(
        lookup: impl Fn(Incarnation) -> Option<String> + Send + 'static,
        mut current_incarnation: impl FnMut(i32) -> Option<Incarnation> + Send + 'static,
        clock: impl MonotonicClock + 'static,
    ) -> Self {
        let clock: Arc<dyn MonotonicClock> = Arc::new(clock);
        let worker_clock = Arc::clone(&clock);
        let state = Arc::new(Mutex::new(State::default()));
        let (queue, requests) = mpsc::sync_channel::<Query>(64);
        let owner = Arc::clone(&state);
        let worker =
            std::thread::Builder::new()
                .name("HAL transport identity".into())
                .spawn(move || {
                    let mut cursor = None;
                    let mut next_reclaim = worker_clock.now().saturating_add(RECLAIM_INTERVAL);
                    loop {
                        let query = match requests
                            .recv_timeout(next_reclaim.duration_since(worker_clock.now()))
                        {
                            Ok(query) => Some(query),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        if worker_clock.now() >= next_reclaim {
                            // One worker owns the cursor. Misses never trigger or
                            // duplicate sweeps, even while discovery is busy.
                            reclaim_step(&owner, &mut cursor, &mut current_incarnation);
                            next_reclaim = worker_clock.now().saturating_add(RECLAIM_INTERVAL);
                        }
                        let Some(query) = query else { continue };
                        let state = lock(&owner);
                        let current = !state.policy.is_empty()
                            && state.identities.get(&query.key).is_some_and(|identity| {
                                Arc::ptr_eq(&identity.answer, &query.answer)
                            });
                        let known = state.known(query.key);
                        drop(state);
                        if current {
                            let discovered = known.or_else(|| lookup(query.key));
                            let mut state = lock(&owner);
                            // Policy changes and client removal retire pending work.
                            if state.identities.get(&query.key).is_some_and(|identity| {
                                Arc::ptr_eq(&identity.answer, &query.answer)
                            }) {
                                state.resolve(query.key, worker_clock.now(), discovered);
                            }
                        }
                        let _ = query.done.try_send(());
                    }
                })
                .ok();
        Self {
            state,
            queue,
            worker,
            inspection: ProcessInspection::SYSTEM,
            clock,
            coreaudio_pid: AtomicI32::new(0),
        }
    }
    pub(crate) fn policy(&self, bundles: Vec<String>) -> bool {
        lock(&self.state).policy(bundles)
    }
    pub(crate) fn client(&self, device: u32, client: u32, pid: i32, bundle: Option<String>) {
        if pid > 0 && bundle.as_deref() == Some(COREAUDIOD_BUNDLE) {
            self.coreaudio_pid.store(pid, Ordering::Release);
        }
        let key = (self.inspection.incarnation)(pid);
        lock(&self.state).client(
            device,
            client,
            Client {
                pid,
                incarnation: key,
                bundle,
            },
        );
    }
    pub(crate) fn remove(&self, device: u32, client: u32) {
        let mut state = lock(&self.state);
        state.remove(device, client);
        let coreaudio = self.coreaudio_pid.load(Ordering::Acquire);
        if coreaudio != 0 && !state.clients.values().any(|client| client.pid == coreaudio) {
            self.coreaudio_pid.store(0, Ordering::Release);
        }
    }
    #[cfg(test)]
    pub(crate) fn get(&self, device: u32, pid: i32) -> u32 {
        self.query(device, pid, QUERY_BUDGET)
    }
    #[cfg(test)]
    fn query(&self, _device: u32, pid: i32, budget: Duration) -> u32 {
        self.get_until(pid, self.clock.now().saturating_add(budget))
    }
    /// Whether a property request comes from coreaudiod itself: PID 0, or the client that
    /// coreaudiod registered under its own bundle. Lock-free, so safe on audio threads.
    pub(crate) fn is_coreaudio(&self, pid: i32) -> bool {
        pid <= 0 || pid == self.coreaudio_pid.load(Ordering::Acquire)
    }
    pub(crate) fn deadline(&self, budget: Duration) -> Instant {
        self.clock.now().saturating_add(budget)
    }
    pub(crate) fn get_until(&self, pid: i32, deadline: Instant) -> u32 {
        self.get_with_incarnation(pid, deadline, self.inspection.incarnation)
    }
    fn get_with_incarnation(
        &self,
        pid: i32,
        deadline: Instant,
        current: impl FnOnce(i32) -> Option<Incarnation>,
    ) -> u32 {
        self.get_with_clock(pid, deadline, current, &SystemClock(self.clock.as_ref()))
    }
    /// One property read against `deadline`, which bounds identity work and discovery alike.
    fn get_with_clock(
        &self,
        pid: i32,
        deadline: Instant,
        current: impl FnOnce(i32) -> Option<Incarnation>,
        clock: &impl Clock,
    ) -> u32 {
        if pid <= 0 || AUDIO_DEPTH.get() != 0 || clock.now() >= deadline {
            return VIRTUAL;
        }
        // Only one fixed-size proc_pidinfo call runs on this non-audio caller.
        // Filesystem discovery and incremental reclamation run on the worker.
        let Some(state) = try_lock(&self.state) else {
            return VIRTUAL;
        };
        if state.policy.is_empty() {
            return VIRTUAL;
        }
        // A registered client answers from the audio server identity. The driver
        // sandbox can refuse process queries, so registration supplies the identity.
        let registered = state
            .registered(pid)
            .map(|(seen, answer)| (seen.incarnation, Arc::clone(&seen.retired), answer));
        drop(state);
        let key = current(pid);
        if let Some((known, retired, answer)) = registered {
            match (known, key) {
                // A new process reusing the PID before its predecessor's registration
                // is removed must not receive the predecessor's answer, now or after.
                (Some(known), Some(key)) if known != key => {
                    retired.store(true, Ordering::Release);
                    if let Some(mut state) = try_lock(&self.state) {
                        state.retire_bundle(pid, &retired);
                    }
                }
                // Inspection that is denied or matches keeps the registered answer,
                // unless it spent the caller's budget.
                _ if clock.now() >= deadline => return VIRTUAL,
                _ => return answer,
            }
        }
        let Some(key) = key else {
            return VIRTUAL;
        };
        self.query_key_until(key, deadline, clock)
    }
    #[cfg(test)]
    fn query_key(&self, key: Incarnation, budget: Duration) -> u32 {
        self.query_key_until(
            key,
            self.clock.now().saturating_add(budget),
            &SystemClock(self.clock.as_ref()),
        )
    }
    fn query_key_until(&self, key: Incarnation, deadline: Instant, clock: &impl Clock) -> u32 {
        let Some(mut state) = try_lock(&self.state) else {
            return VIRTUAL;
        };
        let now = clock.now();
        if now >= deadline {
            return VIRTUAL;
        }
        // At most one cached incarnation per PID. Use its ordered range rather
        // than walking every identity on the property-read path.
        let old = state
            .identities
            .range(Incarnation(key.0, 0, 0)..=Incarnation(key.0, u64::MAX, u64::MAX))
            .next()
            .map(|(old, _)| *old);
        if let Some(old) = old.filter(|old| *old != key) {
            state.identities.remove(&old);
        }
        if state.policy.is_empty() {
            return VIRTUAL;
        }
        if !state.identities.contains_key(&key) {
            if state.identities.len() >= MAX_IDENTITIES {
                // Cleanup belongs to the worker. This refusal is transient,
                // never cached, whether these slots are live or stale.
                return VIRTUAL;
            }
            let known = state.known(key);
            let value = known.as_ref().map_or(PENDING, |bundle| {
                if state.policy.contains(bundle) {
                    USB
                } else {
                    VIRTUAL
                }
            });
            state.identities.insert(
                key,
                Identity {
                    bundle: known,
                    expires: now.saturating_add(NEGATIVE_TTL),
                    answer: Arc::new(AtomicU32::new(value)),
                    in_flight: false,
                },
            );
        }
        if clock.now() >= deadline {
            return VIRTUAL;
        }
        let Some(identity) = state.identities.get_mut(&key) else {
            return VIRTUAL;
        };
        let answer = Arc::clone(&identity.answer);
        let pending = answer.load(Ordering::Acquire) == PENDING;
        let refresh = identity.bundle.is_none() && now >= identity.expires;
        let completion = if (pending || refresh) && !identity.in_flight {
            let (done, completion) = mpsc::sync_channel(1);
            // Mark and enqueue under one lock so concurrent callers cannot
            // enqueue duplicates. try_send never waits for worker capacity.
            identity.in_flight = true;
            if self
                .queue
                .try_send(Query {
                    key,
                    answer: Arc::clone(&answer),
                    done,
                })
                .is_ok()
            {
                Some(completion)
            } else {
                identity.in_flight = false;
                None
            }
        } else {
            None
        };
        drop(state);
        if pending && let Some(completion) = completion {
            let remaining = deadline.duration_since(clock.now());
            if !remaining.is_zero() {
                clock.wait(&completion, remaining);
            }
        }
        match answer.load(Ordering::Acquire) {
            PENDING => VIRTUAL,
            value => value,
        }
    }
}
// Inspect at most RECLAIM_STEP incarnations per worker tick, outside the mutex.
// Compare the answer's ownership before removal so a policy/client change that
// installs a fresh entry for the same key cannot lose it to an old inspection.
fn reclaim_step(
    owner: &Mutex<State>,
    cursor: &mut Option<Incarnation>,
    mut current: impl FnMut(i32) -> Option<Incarnation>,
) {
    let entries: Vec<_> = {
        let state = lock(owner);
        state
            .identities
            .range((cursor.map_or(Unbounded, Excluded), Unbounded))
            .take(RECLAIM_STEP)
            .map(|(key, identity)| (*key, Arc::clone(&identity.answer)))
            .collect()
    };
    *cursor = if entries.len() == RECLAIM_STEP {
        entries.last().map(|(key, _)| *key)
    } else {
        None
    };
    for (key, answer) in entries {
        if current(key.0) != Some(key) {
            let mut state = lock(owner);
            if state
                .identities
                .get(&key)
                .is_some_and(|identity| Arc::ptr_eq(&identity.answer, &answer))
            {
                state.identities.remove(&key);
            }
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        // Disconnect before joining: no worker or process inspection survives Registry.
        let (replacement, _) = mpsc::sync_channel(0);
        drop(std::mem::replace(&mut self.queue, replacement));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
unsafe extern "C" {
    fn mr_hal_log_usb_transport(pid: i32);
    fn mr_hal_identity_start(pid: i32, seconds: *mut u64, micros: *mut u64) -> bool;
    fn mr_hal_identity_bundle(pid: i32, output: *mut c_char, capacity: usize) -> bool;
}
pub(crate) fn report_presentation(pid: i32, value: u32) {
    if value == USB {
        // SAFETY: a scalar diagnostic reaches the native sink only for the non-IO
        // allowlisted requester answer selected by the Rust transport policy.
        unsafe { mr_hal_log_usb_transport(pid) };
    }
}
fn incarnation(pid: i32) -> Option<Incarnation> {
    let (mut seconds, mut micros) = (0, 0);
    unsafe { mr_hal_identity_start(pid, &raw mut seconds, &raw mut micros) }
        .then_some(Incarnation(pid, seconds, micros))
}
fn discover(key: Incarnation) -> Option<String> {
    if incarnation(key.0) != Some(key) {
        return None;
    }
    let value = bundle(key.0);
    (incarnation(key.0) == Some(key)).then_some(value).flatten()
}
fn bundle(pid: i32) -> Option<String> {
    let mut bytes = [0; 128];
    if !unsafe { mr_hal_identity_bundle(pid, bytes.as_mut_ptr(), bytes.len()) } {
        return None;
    }
    CStr::from_bytes_until_nul(unsafe {
        std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len())
    })
    .ok()?
    .to_str()
    .ok()
    .filter(|value| !value.is_empty())
    .map(str::to_owned)
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
