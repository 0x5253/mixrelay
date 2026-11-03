#![cfg_attr(
    any(test, feature = "test-util"),
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "owned simulated native fixture setup fails as a test assertion"
    )
)]
//! A process-local `CoreAudio` behind the production [`Host`] seam. Every operation runs the
//! production transport code beneath the transport's simulated HAL: private-group planning,
//! creation, readback, projection and retirement, and device preparation, listener
//! registration, `IOProc` lifecycle and liveness. Only the `CoreAudio` calls are simulated.
//! Tests plug, unplug, replug or reformat each device explicitly. A connected owner's `IOProc`
//! is the production callback, run once whenever its control worker polls progress, or, while
//! the test holds the callbacks, only in the test's explicit steps.
use super::{
    aggregate::Aggregate,
    buffer_control::BufferInfo,
    callback_invoke::{CallbackTime, invoke},
    ffi,
    group_plan::{GroupPlan, MemberInfo},
    host::{CoreAudio, Host},
    native::Native,
    spec::Spec,
};
use mixrelay_audio_io::{AudioDeviceDescriptor, DeviceSelector, HostAudioBackendError};
use mixrelay_macos_transport::{
    aggregate_group::{NativeBinding, test_observation as aggregate},
    device_io::test_observation::{self as device_io, SimulatedHal},
};
use std::{
    collections::{BTreeMap, VecDeque},
    ffi::{CStr, c_void},
    ptr::NonNull,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

pub use device_io::DEFAULT_BUFFER;
/// The finite PCM a simulated microphone captures, unless a test plugs another level.
pub const CAPTURED: f32 = 0.25;

#[derive(Default)]
pub struct SimulatedHost {
    hal: SimulatedHal,
    state: Mutex<State>,
    /// Serializes `IOProc` invocations with each other and with unregistration, as `CoreAudio`
    /// drains a callback before unregister returns. True while the test holds the callbacks.
    callbacks: Mutex<bool>,
    /// Progress polls of every control worker so far.
    polls: AtomicU64,
    consent: Mutex<VecDeque<super::permission::InputPermission>>,
    consent_requests: AtomicU64,
    consent_observations: Mutex<Vec<bool>>,
    permission_waiting: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct State {
    /// Each native owner's direction, by its stable allocation address.
    owners: BTreeMap<usize, bool>,
    /// Each registration's device clock, by procedure identity.
    timelines: BTreeMap<usize, Timeline>,
}

/// While held, workers no longer run callbacks: only [`SimulatedHost::callback`] does.
pub(crate) struct HeldCallbacks<'a>(&'a SimulatedHost);

impl Drop for HeldCallbacks<'_> {
    fn drop(&mut self) {
        *self.0.callbacks() = false;
    }
}

/// A device clock that starts now and advances exactly by the frames its callbacks transfer.
#[derive(Clone, Copy)]
struct Timeline {
    rate: u32,
    start: u64,
    sample: u64,
}

impl SimulatedHost {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    #[cfg(test)]
    pub(super) fn consent_decisions(
        &self,
        decisions: impl IntoIterator<Item = super::permission::InputPermission>,
    ) {
        *self.consent.lock().unwrap() = decisions.into_iter().collect();
    }

    #[cfg(test)]
    pub(crate) fn consent_observations(&self) -> Vec<bool> {
        self.consent_observations.lock().unwrap().clone()
    }

    #[cfg(test)]
    pub(crate) fn consent_requests(&self) -> u64 {
        self.consent_requests.load(Ordering::Relaxed)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn callbacks(&self) -> MutexGuard<'_, bool> {
        self.callbacks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a present duplex device of `channels` per direction at its native `rate`,
    /// capturing `capture` on every input sample.
    pub fn plug(&self, uid: &str, channels: u16, rate: u32, capture: f32) -> String {
        self.hal.plug(uid, channels, rate, capture);
        uid.into()
    }

    /// Removes the device: its listeners fire, and registrations and queries of it fail.
    pub fn unplug(&self, uid: &str) {
        self.hal.unplug(uid);
    }

    /// The same device reappears as a new object. Returns the client buffer it reappeared
    /// with, observed as it is published, before any worker can reconnect.
    /// # Panics
    /// Panics if this fixture never plugged the named device.
    pub fn replug(&self, uid: &str) -> u32 {
        self.hal.replug(uid).expect("simulated device")
    }

    /// Every later connection attempt finds a stream format no client can admit.
    pub fn publish_unsupported_format(&self, uid: &str) {
        self.hal.publish_unsupported_format(uid);
    }

    /// Reads this fixture device's current client buffer.
    /// # Panics
    /// Panics if this fixture never plugged the named device.
    pub fn buffer(&self, uid: &str) -> u32 {
        self.hal.buffer(uid).expect("simulated device")
    }

    /// How many times control workers polled callback progress.
    pub fn polls(&self) -> u64 {
        self.polls.load(Ordering::Acquire)
    }

    /// `IOProc` registrations the HAL has not seen destroyed.
    pub fn registrations(&self) -> usize {
        self.hal.registrations()
    }

    /// The aggregate device of every live private group, in creation order.
    pub fn groups(&self) -> Vec<u32> {
        self.hal.aggregates()
    }

    /// Stops worker-driven callbacks until the returned guard drops; a callback in flight
    /// completes first. The test then runs each callback with [`Self::callback`].
    pub fn hold_callbacks(&self) -> impl Sized + '_ {
        *self.callbacks() = true;
        HeldCallbacks(self)
    }

    /// Runs one callback of the owner's started registration and returns its interleaved
    /// buffer as the `IOProc` left it: the captured input or the rendered output.
    /// None while the owner has no started registration on a live device.
    pub(super) fn callback(&self, native: &Native) -> Option<Vec<f32>> {
        let _serial = self.callbacks();
        self.serve(native)
    }

    /// Runs one callback of the owner's started registration on the supplied interleaved
    /// `pcm`: the input the device captured, or the storage the output renders into.
    /// False, leaving `pcm` untouched, while the owner has no started registration on a
    /// live device or `pcm` is not one whole buffer of that stream.
    #[cfg(test)]
    pub(super) fn callback_on(&self, native: &Native, pcm: &mut [f32]) -> bool {
        let _serial = self.callbacks();
        let io = native.pointer.as_ptr();
        let Some((stream, input)) = self.started(io) else {
            return false;
        };
        if pcm.len() != stream.capture.len() * stream.frames as usize {
            return false;
        }
        self.run(io, input, &stream, pcm);
        true
    }

    /// One callback of a started registration. The caller holds the callback lock.
    fn serve(&self, native: &Native) -> Option<Vec<f32>> {
        let io = native.pointer.as_ptr();
        let (stream, input) = self.started(io)?;
        let mut pcm = if input {
            stream.capture.repeat(stream.frames as usize)
        } else {
            vec![0.0; stream.capture.len() * stream.frames as usize]
        };
        self.run(io, input, &stream, &mut pcm);
        Some(pcm)
    }

    /// The stream and direction of the owner's started registration on a live device.
    fn started(&self, io: *mut c_void) -> Option<(device_io::Stream, bool)> {
        let stream = self.hal.stream(io.cast())?;
        let input = *self.state().owners.get(&(io as usize))?;
        Some((stream, input))
    }

    /// Invokes the started registration's `IOProc` on `pcm` at its device clock, then
    /// advances that clock. The caller holds the callback lock.
    fn run(&self, io: *mut c_void, input: bool, stream: &device_io::Stream, pcm: &mut [f32]) {
        let time = self
            .state()
            .timelines
            .entry(stream.procedure)
            .or_insert_with(|| Timeline::new(stream.rate))
            .time(input, stream.frames);
        // SAFETY: the callback lock serializes this owner's callbacks with each other and with
        // unregistration; its started registration admitted this Float32 stream width, and
        // `pcm` holds exactly one buffer of it.
        unsafe { invoke(io, input, stream.capture.len(), pcm, time) };
        if let Some(timeline) = self.state().timelines.get_mut(&stream.procedure) {
            timeline.advance(stream.frames);
        }
    }
}

impl Timeline {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            start: mixrelay_realtime::host_time(),
            sample: 0,
        }
    }

    fn advance(&mut self, frames: u32) {
        self.sample += u64::from(frames);
    }

    /// The next callback of `frames`: capture of its first frame for input, presentation for output.
    #[expect(
        clippy::cast_precision_loss,
        reason = "simulated timelines stay far below 2^52 frames"
    )]
    fn time(self, input: bool, frames: u32) -> CallbackTime {
        let timebase = mixrelay_realtime::Timebase::current();
        let at = |sample: u64| {
            self.start
                + timebase.ticks(Duration::from_nanos(
                    sample * 1_000_000_000 / u64::from(self.rate),
                ))
        };
        let first = at(self.sample);
        let next = at(self.sample + u64::from(frames));
        CallbackTime {
            // Input arrives once captured; output presents one buffer after the callback.
            now: if input { next } else { first },
            host: if input { first } else { next },
            sample: self.sample as f64,
            rate_scalar: 1.0,
        }
    }
}

impl Host for SimulatedHost {
    fn set_permission_waiting(&self, waiting: bool) {
        self.permission_waiting.store(waiting, Ordering::Relaxed);
    }
    fn permission_waiting(&self) -> bool {
        self.permission_waiting.load(Ordering::Relaxed)
    }
    fn input_permission(&self) -> super::permission::InputPermission {
        self.consent
            .lock()
            .unwrap()
            .front()
            .copied()
            .unwrap_or(super::permission::InputPermission::Authorized)
    }
    fn request_input_permission(&self) {
        self.consent_requests.fetch_add(1, Ordering::Relaxed);
    }
    fn poll_input_permission(&self) -> super::permission::InputPermission {
        self.consent_observations
            .lock()
            .unwrap()
            .push(self.permission_waiting());
        let mut consent = self.consent.lock().unwrap();
        if consent.len() > 1 {
            consent.pop_front();
        }
        consent
            .front()
            .copied()
            .unwrap_or(super::permission::InputPermission::Authorized)
    }
    fn inventory(
        &self,
        selectors: &[&DeviceSelector],
    ) -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
        let mut present = BTreeMap::new();
        for selector in selectors {
            if let DeviceSelector::PersistentUid { uid } = selector
                && let Some(channels) = self.hal.published(uid)
            {
                present.insert(
                    uid.clone(),
                    AudioDeviceDescriptor {
                        uid: uid.clone(),
                        display_name: uid.clone(),
                        input_channels: channels,
                        output_channels: channels,
                        is_default_input: false,
                        is_default_output: false,
                    },
                );
            }
        }
        Ok(present.into_values().collect())
    }

    fn member_info(&self, uid: &CStr) -> Option<MemberInfo> {
        let mut info = MemberInfo::default();
        // SAFETY: a terminated UID borrowed for the call and one exclusive repr(C) snapshot
        // with the transport's member layout.
        unsafe { aggregate::member_info(&self.hal, uid.as_ptr(), (&raw mut info).cast()) }
            .then_some(info)
    }

    fn create_group(&self, plan: &GroupPlan) -> (Option<NonNull<c_void>>, i32) {
        let names = plan
            .names
            .iter()
            .map(|uid| uid.as_ptr())
            .collect::<Vec<_>>();
        if names.len() != plan.facts.len() {
            return (None, 1);
        }
        // SAFETY: one initialized snapshot with the transport's member layout per terminated
        // UID, both immutable for the call. Any returned owner transfers to us.
        let (group, status) =
            unsafe { aggregate::create(&self.hal, &names, plan.facts.as_ptr().cast(), 0) };
        (NonNull::new(group.cast()), status)
    }

    fn group_ready(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: the Aggregate retains this owner, excluding retirement during the query.
        unsafe { aggregate::ready(&self.hal, group.as_ptr().cast()) }
    }

    fn destroy_group(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: called only from retirement after every registration released the group.
        unsafe { aggregate::try_destroy(&self.hal, group.as_ptr().cast()) }
    }

    fn create(
        &self,
        spec: &Spec,
        uid: &CStr,
        rate: u32,
        group: Option<&Aggregate>,
    ) -> Option<NonNull<c_void>> {
        // Allocation is the production one and makes no CoreAudio call.
        let pointer = CoreAudio::default().create(spec, uid, rate, group)?;
        self.state()
            .owners
            .insert(pointer.as_ptr() as usize, spec.input);
        Some(pointer)
    }

    fn buffer_info(&self, native: &Native, info: &mut BufferInfo) -> bool {
        // SAFETY: Native retains its owner; `info` is exclusive storage with the transport's
        // buffer snapshot layout.
        unsafe {
            device_io::buffer_info(
                &self.hal,
                native.pointer.as_ptr().cast(),
                std::ptr::from_mut(info).cast(),
            )
        }
    }

    fn buffer_size(&self, device: u32) -> u32 {
        device_io::buffer_size(&self.hal, device)
    }

    fn write_buffer(&self, device: u32, frames: u32) -> bool {
        device_io::buffer_write(&self.hal, device, frames)
    }

    fn connect(&self, native: &Native, expected: u32) -> i32 {
        // SAFETY: Native retains its owner and its sole control worker serializes registration.
        unsafe { device_io::connect_status(&self.hal, native.pointer.as_ptr().cast(), expected) }
    }

    fn disconnect(&self, native: &Native) -> bool {
        // Unregistration drains: no callback of this owner runs until it returns.
        let _serial = self.callbacks();
        // SAFETY: the sole control worker unregisters; failure keeps the owner allocated.
        unsafe { device_io::disconnect(&self.hal, native.pointer.as_ptr().cast()) }
    }

    fn destroy(&self, native: &Native) -> bool {
        let _serial = self.callbacks();
        let pointer = native.pointer.as_ptr();
        // SAFETY: Native's drop is the sole remaining owner; workers and graph roles stopped.
        let destroyed = unsafe { device_io::destroy(&self.hal, pointer.cast()) };
        if destroyed {
            self.state().owners.remove(&(pointer as usize));
        }
        destroyed
    }

    fn direct_binding(&self, native: &Native) -> Option<NativeBinding> {
        // SAFETY: Native retains its owner for this immutable-selection query.
        unsafe { device_io::direct_binding(&self.hal, native.pointer.as_ptr().cast()) }
    }

    fn alive(&self, native: &Native) -> bool {
        // SAFETY: a control-only query of the retained owner, serialized with registration.
        unsafe { device_io::alive(&self.hal, native.pointer.as_ptr().cast()) }
    }

    fn clock_device(&self, native: &Native) -> u32 {
        // SAFETY: a control-only query of the retained owner, serialized with registration.
        unsafe { device_io::clock_device(&self.hal, native.pointer.as_ptr().cast()) }
    }

    fn ticks(&self, native: &Native) -> u64 {
        let held = self.callbacks();
        if !*held {
            self.serve(native);
        }
        drop(held);
        // Release: a test that observes this poll also observes any callback it ran.
        self.polls.fetch_add(1, Ordering::Release);
        // SAFETY: the owner is retained by the caller; this is an atomic progress read.
        unsafe { ffi::mr_device_io_ticks(native.pointer.as_ptr()) }
    }
}
