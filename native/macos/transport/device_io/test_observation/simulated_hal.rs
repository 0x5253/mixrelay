//! A process-local `CoreAudio` HAL beneath the production device and aggregate code.
//!
//! Devices, private aggregates, stream objects, property listeners and `IOProc`
//! registrations are explicit simulated objects. Production preparation, admission,
//! listener installation, lifecycle and retirement run unchanged above this boundary;
//! no installed device, OS registration or environment selection is involved.
//! Tests plug, unplug, replug or reformat a device explicitly. A replugged device
//! returns as a new object with its default client buffer, as `CoreAudio` republishes it.

use std::{
    collections::BTreeMap,
    ffi::{CStr, c_void},
    ptr::{self, NonNull},
    sync::{Mutex, MutexGuard, PoisonError},
};

use block2::{Block, RcBlock};

use super::super::{
    DeviceIo,
    control::Hal,
    lifecycle::Created,
    listeners::{ListenerBackend, Subscription, WatchTarget},
    native::{Address, Description, Properties, StreamIds},
    stream_usage::{Backend as UsageBackend, Usage},
};
use crate::aggregate_group::{NativeBinding, test_observation as aggregate};

#[cfg(test)]
mod tests;

/// Client buffer a device has whenever it (re)appears.
pub const DEFAULT_BUFFER: u32 = 512;
/// The client buffer range every simulated device accepts.
const BUFFER_RANGE: (u32, u32) = (15, 4096);
const BAD_DEVICE: i32 = i32::from_be_bytes(*b"!dev");
const BAD_OBJECT: i32 = i32::from_be_bytes(*b"!obj");
const ILLEGAL_OPERATION: i32 = i32::from_be_bytes(*b"nope");
const UNKNOWN_PROPERTY: i32 = i32::from_be_bytes(*b"who?");

type Notification = dyn Fn(u32, *const c_void);

/// The simulated HAL. Every query and registration of the production code goes here.
#[derive(Default)]
pub struct SimulatedHal {
    state: Mutex<State>,
}

/// One started registration's stream as its device presents it to the `IOProc`.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    /// The registration's procedure identity, unique for the HAL's lifetime.
    pub procedure: usize,
    pub rate: u32,
    /// The device's current client buffer, the frames of each callback.
    pub frames: u32,
    /// One captured frame of the device's whole input stream; its length is the stream width.
    pub capture: Vec<f32>,
}

#[derive(Default)]
struct State {
    usage_status: i32,
    /// Last allocated object identity; zero is never an object.
    objects: u32,
    devices: BTreeMap<String, Device>,
    aggregates: BTreeMap<u32, Aggregate>,
    procedures: BTreeMap<usize, Procedure>,
    /// Last allocated procedure identity.
    registrations: usize,
    watches: Vec<Watch>,
}

struct Device {
    id: u32,
    /// Input and output stream objects.
    streams: [u32; 2],
    channels: u32,
    rate: u32,
    buffer: u32,
    present: bool,
    /// The level every captured sample of this device has.
    capture: f32,
    /// False publishes a stream format no client can admit.
    supported: bool,
}

/// A published private aggregate over physical members, in composition order.
struct Aggregate {
    streams: [u32; 2],
    members: Vec<(String, i32)>,
    master: String,
    buffer: u32,
}

/// The facts of one aggregate the HAL republishes in its readback.
pub(crate) struct Composition {
    /// Member UIDs with their drift-compensation flag, in composition order.
    pub(crate) members: Vec<(String, i32)>,
    pub(crate) master: String,
}

struct Procedure {
    device: u32,
    /// The `IOProc` context, a retained device owner.
    context: usize,
    started: bool,
    /// Usage per direction, independently owned by this registration.
    enabled: [bool; 2],
}

/// One registered listener block, retained like `CoreAudio`'s own copy.
struct Watch {
    subscription: Subscription,
    block: RcBlock<Notification>,
}

// SAFETY: production listener blocks capture only an `Arc<AtomicBool>` latch, which
// `CoreAudio` itself retains, invokes and releases from arbitrary threads.
unsafe impl Send for Watch {}

enum Object<'a> {
    Device(&'a Device),
    Aggregate(&'a Aggregate),
    /// A stream object of `device`.
    Stream(u32),
}

impl SimulatedHal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publishes a present duplex device of `channels` per direction at its nominal `rate`,
    /// capturing `capture` on every input sample. Returns its object identity.
    pub fn plug(&self, uid: &str, channels: u16, rate: u32, capture: f32) -> u32 {
        let mut state = self.state();
        let [id, input, output] = state.allocate();
        state.devices.insert(
            uid.into(),
            Device {
                id,
                streams: [input, output],
                channels: u32::from(channels),
                rate,
                buffer: DEFAULT_BUFFER,
                present: true,
                capture,
                supported: true,
            },
        );
        id
    }

    /// Removes the device object: queries and registrations of it fail, and listeners of it,
    /// its streams and every aggregate over it are notified, as when hardware disappears.
    pub fn unplug(&self, uid: &str) {
        let notified = {
            let mut state = self.state();
            let Some(device) = state.devices.get_mut(uid) else {
                return;
            };
            device.present = false;
            let mut objects = vec![device.id, device.streams[0], device.streams[1]];
            // Every aggregate over the device loses a subdevice and its stream layout.
            for (&id, aggregate) in &state.aggregates {
                if aggregate.members.iter().any(|(member, _)| member == uid) {
                    objects.extend([id, aggregate.streams[0], aggregate.streams[1]]);
                }
            }
            state
                .watches
                .iter()
                .filter(|watch| objects.contains(&watch.subscription.target().0))
                .map(|watch| (watch.subscription.target().0, watch.block.clone()))
                .collect::<Vec<_>>()
        };
        for (object, block) in notified {
            block.call((object, ptr::null()));
        }
    }

    /// The same UID reappears as a new object with its default client buffer. Returns the
    /// buffer it reappeared with, observed in the same step that publishes it.
    pub fn replug(&self, uid: &str) -> Option<u32> {
        let mut state = self.state();
        let [id, input, output] = state.allocate();
        let device = state.devices.get_mut(uid)?;
        device.id = id;
        device.streams = [input, output];
        device.buffer = DEFAULT_BUFFER;
        device.present = true;
        Some(device.buffer)
    }

    /// From now on the device publishes a compressed stream format.
    pub fn publish_unsupported_format(&self, uid: &str) {
        if let Some(device) = self.state().devices.get_mut(uid) {
            device.supported = false;
        }
    }

    /// `IOProc` registrations not yet destroyed.
    #[must_use]
    pub fn registrations(&self) -> usize {
        self.state().procedures.len()
    }

    /// Channels per direction of a present device, or None while it is absent.
    #[must_use]
    pub fn published(&self, uid: &str) -> Option<u32> {
        let state = self.state();
        let device = state.devices.get(uid)?;
        device.present.then_some(device.channels)
    }

    /// The device's current client buffer.
    #[must_use]
    pub fn buffer(&self, uid: &str) -> Option<u32> {
        self.state().devices.get(uid).map(|device| device.buffer)
    }

    /// The identity of every published aggregate, in creation order.
    #[must_use]
    pub fn aggregates(&self) -> Vec<u32> {
        self.state().aggregates.keys().copied().collect()
    }

    /// The stream of `io`'s started registration on a live device, if any.
    #[must_use]
    pub fn stream(&self, io: *const DeviceIo) -> Option<Stream> {
        let state = self.state();
        let (&procedure, registration) = state.procedures.iter().find(|(_, registration)| {
            registration.context == io as usize && registration.started
        })?;
        let (rate, frames, capture) = match state.object(registration.device)? {
            Object::Device(device) if device.present => (
                device.rate,
                device.buffer,
                vec![device.capture; usize::try_from(device.channels).ok()?],
            ),
            Object::Aggregate(aggregate) => {
                let mut capture = Vec::new();
                for (uid, _) in &aggregate.members {
                    let member = state.devices.get(uid).filter(|member| member.present)?;
                    let width = usize::try_from(member.channels).ok()?;
                    capture.extend(std::iter::repeat_n(member.capture, width));
                }
                (state.rate(registration.device)?, aggregate.buffer, capture)
            }
            _ => return None,
        };
        Some(Stream {
            procedure,
            rate,
            frames,
            capture,
        })
    }

    /// Listener blocks currently registered.
    #[cfg(test)]
    pub(super) fn watches(&self) -> usize {
        self.state().watches.len()
    }

    /// Every present physical device with its object identity.
    pub(crate) fn devices(&self) -> Vec<(String, u32)> {
        self.state()
            .devices
            .iter()
            .filter(|(_, device)| device.present)
            .map(|(uid, device)| (uid.clone(), device.id))
            .collect()
    }

    /// One global u32 property: transport type or liveness.
    pub(crate) fn scalar(&self, id: u32, selector: [u8; 4]) -> Option<u32> {
        self.state().scalar(id, selector)
    }

    pub(crate) fn rate(&self, id: u32) -> Option<f64> {
        self.state().rate(id).map(f64::from)
    }

    /// Total channels of one direction's stream.
    pub(crate) fn channels(&self, id: u32) -> Option<u32> {
        self.state().channels(id)
    }

    pub(crate) fn composition(&self, id: u32) -> Option<Composition> {
        let state = self.state();
        let aggregate = state.aggregates.get(&id)?;
        Some(Composition {
            members: aggregate.members.clone(),
            master: aggregate.master.clone(),
        })
    }

    /// The active subdevices of an aggregate: all of its members, or None once one is gone.
    pub(crate) fn active(&self, id: u32) -> Option<Vec<u32>> {
        let state = self.state();
        state
            .aggregates
            .get(&id)?
            .members
            .iter()
            .map(|(uid, _)| {
                state
                    .devices
                    .get(uid)
                    .filter(|device| device.present)
                    .map(|device| device.id)
            })
            .collect()
    }

    /// Publishes a private aggregate of present members; a native status otherwise.
    pub(crate) fn create_aggregate(&self, composition: Composition) -> Result<u32, i32> {
        let mut state = self.state();
        let present = |uid: &String| state.devices.get(uid).is_some_and(|device| device.present);
        if composition.members.len() < 2
            || !composition.members.iter().all(|(uid, _)| present(uid))
            || !composition
                .members
                .iter()
                .any(|(uid, _)| *uid == composition.master)
        {
            return Err(BAD_DEVICE);
        }
        let [id, input, output] = state.allocate();
        state.aggregates.insert(
            id,
            Aggregate {
                streams: [input, output],
                members: composition.members,
                master: composition.master,
                buffer: DEFAULT_BUFFER,
            },
        );
        Ok(id)
    }

    pub(crate) fn destroy_aggregate(&self, id: u32) -> i32 {
        let mut state = self.state();
        if state
            .procedures
            .values()
            .any(|registration| registration.device == id)
        {
            // A registered IOProc still uses the aggregate.
            return ILLEGAL_OPERATION;
        }
        if state.aggregates.remove(&id).is_some() {
            0
        } else {
            BAD_DEVICE
        }
    }
}

impl State {
    /// A device and its input and output stream objects.
    fn allocate(&mut self) -> [u32; 3] {
        // Object 1 is the permanent AudioSystemObject, not a device.
        let first = self.objects.max(1) + 1;
        self.objects = first + 2;
        [first, first + 1, first + 2]
    }

    fn object(&self, id: u32) -> Option<Object<'_>> {
        for device in self.devices.values().filter(|device| device.present) {
            if device.id == id {
                return Some(Object::Device(device));
            }
            if device.streams.contains(&id) {
                return Some(Object::Stream(device.id));
            }
        }
        self.aggregates.iter().find_map(|(&aggregate, state)| {
            if aggregate == id {
                return Some(Object::Aggregate(state));
            }
            state
                .streams
                .contains(&id)
                .then_some(Object::Stream(aggregate))
        })
    }

    /// Whether the object exists and every physical device beneath it is present.
    fn alive(&self, id: u32) -> bool {
        match self.object(id) {
            Some(Object::Device(_)) => true,
            Some(Object::Aggregate(aggregate)) => aggregate
                .members
                .iter()
                .all(|(uid, _)| self.devices.get(uid).is_some_and(|device| device.present)),
            Some(Object::Stream(device)) => self.alive(device),
            None => false,
        }
    }

    fn scalar(&self, id: u32, selector: [u8; 4]) -> Option<u32> {
        match (&selector, self.object(id)?) {
            (b"livn", Object::Device(_) | Object::Aggregate(_)) => Some(u32::from(self.alive(id))),
            (b"tran", Object::Device(_)) => Some(u32::from_be_bytes(*b"bltn")),
            (b"tran", Object::Aggregate(_)) => Some(u32::from_be_bytes(*b"grup")),
            (b"fsiz", Object::Device(device)) => Some(device.buffer),
            (b"fsiz", Object::Aggregate(aggregate)) => Some(aggregate.buffer),
            _ => None,
        }
    }

    fn rate(&self, id: u32) -> Option<u32> {
        match self.object(id)? {
            Object::Device(device) => Some(device.rate),
            Object::Aggregate(aggregate) => self
                .devices
                .get(&aggregate.master)
                .filter(|master| master.present)
                .map(|master| master.rate),
            Object::Stream(device) => self.rate(device),
        }
    }

    /// Channels per direction; every simulated member is symmetric duplex.
    fn channels(&self, id: u32) -> Option<u32> {
        match self.object(id)? {
            Object::Device(device) => Some(device.channels),
            Object::Aggregate(aggregate) => {
                aggregate
                    .members
                    .iter()
                    .try_fold(0, |total: u32, (uid, _)| {
                        let member = self.devices.get(uid).filter(|member| member.present)?;
                        total.checked_add(member.channels)
                    })
            }
            Object::Stream(device) => self.channels(device),
        }
    }

    /// The stream's current physical format: packed Float32, or compressed when unsupported.
    fn format(&self, stream: u32) -> Option<Description> {
        let Object::Stream(device) = self.object(stream)? else {
            return None;
        };
        let supported = match self.object(device)? {
            Object::Device(device) => device.supported,
            Object::Aggregate(aggregate) => aggregate
                .members
                .iter()
                .all(|(uid, _)| self.devices.get(uid).is_some_and(|member| member.supported)),
            Object::Stream(_) => return None,
        };
        let channels = self.channels(device)?;
        Some(Description {
            rate: f64::from(self.rate(device)?),
            format: u32::from_be_bytes(if supported { *b"lpcm" } else { *b"ac-3" }),
            // Float, packed.
            flags: 0x09,
            packet_bytes: channels * 4,
            packet_frames: 1,
            frame_bytes: channels * 4,
            channels,
            bits: 32,
            reserved: 0,
        })
    }

    fn write_buffer(&mut self, id: u32, frames: u32) -> bool {
        if !self.alive(id) || !(BUFFER_RANGE.0..=BUFFER_RANGE.1).contains(&frames) {
            return false;
        }
        if let Some(aggregate) = self.aggregates.get_mut(&id) {
            aggregate.buffer = frames;
            return true;
        }
        self.devices
            .values_mut()
            .find(|device| device.present && device.id == id)
            .is_some_and(|device| {
                device.buffer = frames;
                true
            })
    }
}

/// `value` as the caller's property type, when both have the same size.
fn property<T: Copy + Default, V: Copy>(value: V) -> Option<T> {
    // SAFETY: the transport reads only plain-old-data ABI types (u32, f64 ranges and stream
    // descriptions), every bit pattern of which is valid; the sizes are equal.
    (size_of::<T>() == size_of::<V>()).then(|| unsafe { std::mem::transmute_copy(&value) })
}

impl Properties for SimulatedHal {
    fn read<T: Copy + Default>(&self, id: u32, address: Address) -> Option<T> {
        let state = self.state();
        let selector = address.selector.to_be_bytes();
        match &selector {
            b"livn" | b"tran" | b"fsiz" => property(state.scalar(id, selector)?),
            b"nsrt" => property(f64::from(state.rate(id)?)),
            b"fsz#" if state.alive(id) => {
                property([f64::from(BUFFER_RANGE.0), f64::from(BUFFER_RANGE.1)])
            }
            b"sfmt" => property(state.format(id)?),
            _ => None,
        }
    }

    fn streams(&self, id: u32, scope: u32, limit: u32) -> Option<StreamIds> {
        let state = self.state();
        let streams = match state.object(id)? {
            Object::Device(device) => device.streams,
            Object::Aggregate(aggregate) => aggregate.streams,
            Object::Stream(_) => return None,
        };
        let input = match &scope.to_be_bytes() {
            b"inpt" => true,
            b"outp" => false,
            _ => return None,
        };
        if limit == 0 {
            return None;
        }
        let mut values = [0; 2048];
        values[0] = streams[usize::from(!input)];
        Some(StreamIds { values, count: 1 })
    }

    fn device(&self, uid: &CStr) -> Option<u32> {
        let state = self.state();
        let device = state.devices.get(uid.to_str().ok()?)?;
        device.present.then_some(device.id)
    }

    unsafe fn group_binding(
        &self,
        group: NonNull<c_void>,
        uid: &CStr,
        input: bool,
        channels: u32,
    ) -> Option<NativeBinding> {
        // SAFETY: the caller retains the aggregate owner through this query.
        unsafe { aggregate::binding(self, group.as_ptr().cast(), uid, input, channels) }
    }

    unsafe fn group_ready(&self, group: NonNull<c_void>) -> bool {
        // SAFETY: the caller retains the aggregate owner through this query.
        unsafe { aggregate::ready(self, group.as_ptr().cast()) }
    }

    fn settable(&self, id: u32, address: Address) -> bool {
        address.selector == u32::from_be_bytes(*b"fsiz") && self.state().alive(id)
    }

    fn write(&self, id: u32, address: Address, value: u32) -> bool {
        address.selector == u32::from_be_bytes(*b"fsiz") && self.state().write_buffer(id, value)
    }
}

impl ListenerBackend for SimulatedHal {
    fn add(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        let mut state = self.state();
        if subscription.target().0 != 1 && state.object(subscription.target().0).is_none() {
            return BAD_OBJECT;
        }
        state.watches.push(Watch {
            subscription,
            block: block.copy(),
        });
        0
    }

    fn remove(&self, subscription: Subscription, block: &Block<Notification>) -> i32 {
        let mut state = self.state();
        let Some(index) = state.watches.iter().position(|watch| {
            watch.subscription == subscription
                && ptr::eq::<Block<Notification>>(&raw const *watch.block, block)
        }) else {
            return UNKNOWN_PROPERTY;
        };
        state.watches.swap_remove(index);
        if subscription.target().0 == 1 || state.object(subscription.target().0).is_some() {
            0
        } else {
            BAD_OBJECT
        }
    }

    fn streams(&self, target: &WatchTarget) -> Option<StreamIds> {
        Properties::streams(self, target.device, target.scope, target.limit)
    }
}

impl Hal for SimulatedHal {
    unsafe fn create(&self, device: u32, context: NonNull<c_void>) -> Created {
        let mut state = self.state();
        if !state.alive(device) {
            return Created {
                procedure: None,
                status: BAD_DEVICE,
            };
        }
        state.registrations += 1;
        let procedure = state.registrations;
        state.procedures.insert(
            procedure,
            Procedure {
                device,
                context: context.as_ptr() as usize,
                started: false,
                enabled: [true; 2],
            },
        );
        Created {
            // An opaque identity, never dereferenced.
            procedure: NonNull::new(ptr::without_provenance_mut(procedure)),
            status: 0,
        }
    }

    fn start(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        let mut state = self.state();
        let alive = state.alive(device);
        match state.procedures.get_mut(&procedure.as_ptr().addr()) {
            Some(registration) if registration.device == device && alive => {
                registration.started = true;
                0
            }
            Some(_) => BAD_DEVICE,
            None => ILLEGAL_OPERATION,
        }
    }

    fn stop(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        let mut state = self.state();
        let alive = state.alive(device);
        let Some(registration) = state.procedures.get_mut(&procedure.as_ptr().addr()) else {
            return ILLEGAL_OPERATION;
        };
        registration.started = false;
        if alive { 0 } else { BAD_DEVICE }
    }

    fn destroy(&self, device: u32, procedure: NonNull<c_void>) -> i32 {
        let mut state = self.state();
        let alive = state.object(device).is_some();
        if state
            .procedures
            .remove(&procedure.as_ptr().addr())
            .is_none()
        {
            return ILLEGAL_OPERATION;
        }
        if alive { 0 } else { BAD_DEVICE }
    }
}

impl UsageBackend for SimulatedHal {
    fn write_usage(&self, device: u32, scope: u32, usage: &Usage, bytes: u32) -> i32 {
        let direction = match &scope.to_be_bytes() {
            b"inpt" => 0,
            b"outp" => 1,
            _ => return UNKNOWN_PROPERTY,
        };
        let mut state = self.state();
        if state.usage_status != 0 {
            return state.usage_status;
        }
        if !state.alive(device) {
            return BAD_DEVICE;
        }
        let Some(registration) = state.procedures.get_mut(&usage.procedure.addr()) else {
            return ILLEGAL_OPERATION;
        };
        if registration.device != device || usage.count != 1 || bytes != 16 {
            return ILLEGAL_OPERATION;
        }
        registration.enabled[direction] = usage.enabled[0] != 0;
        0
    }

    fn read_usage(&self, device: u32, scope: u32, usage: &mut Usage, bytes: &mut u32) -> i32 {
        let direction = match &scope.to_be_bytes() {
            b"inpt" => 0,
            b"outp" => 1,
            _ => return UNKNOWN_PROPERTY,
        };
        let state = self.state();
        if !state.alive(device) {
            return BAD_DEVICE;
        }
        let Some(registration) = state.procedures.get(&usage.procedure.addr()) else {
            return ILLEGAL_OPERATION;
        };
        if registration.device != device || *bytes != 16 {
            return ILLEGAL_OPERATION;
        }
        usage.count = 1;
        usage.enabled[0] = u32::from(registration.enabled[direction]);
        0
    }
}
