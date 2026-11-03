//! Control-owned client PCM queues and capture mixing independent of postmix callbacks.
//!
//! A queue exists only for a client that may render: one added while the device runs,
//! or one that starts IO. Configuration changes never map storage. A geometry change
//! detaches every queue of the old shape and a rate rearm detaches every queue without
//! a spare epoch; each client maps a fresh queue at its next `StartIO`, on control and
//! outside the freeze. Detached queues are handed to the caller, which unmaps them
//! after the freeze ends.
use crate::{abi::Geometry, clock::Clock, io_outcome::IoOutcome};
use mixrelay_macos_transport::{
    client_pcm::{
        ClientPcmClockOwner, ClientPcmSnapshot, ClientPcmTimeline, ClientPcmWriter as Queue,
        MAX_CLIENTS_PER_ENDPOINT as MAX_CLIENTS, MAX_PCM_BYTES_PER_ENDPOINT as MAX_PCM_BYTES,
        QUEUE_FRAMES,
    },
    endpoint_transport::EndpointTransportConfig,
    shared_audio_buffer as buffer,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

// A copy tile bounds stack scratch to 8 KiB at the supported 64-channel width.
// It does not change the device quantum, timeline or queue capacity.
const COPY_FRAMES: usize = 32;
const MAX_CHANNELS: usize = 64;
/// Detached queues one freeze can hand back without growing the retired list.
pub(super) const RETIRED_CAPACITY: usize = MAX_CLIENTS;

/// Clients whose queues of `channels` fit in `budget` bytes, at most `MAX_CLIENTS`.
fn client_limit(budget: usize, channels: u32) -> usize {
    if channels == 0 {
        return MAX_CLIENTS;
    }
    let bytes = buffer::mr_shared_audio_buffer_required_bytes(QUEUE_FRAMES, channels);
    budget
        .checked_div(bytes)
        .map_or(0, |limit| limit.min(MAX_CLIENTS))
}

/// Hands a detached queue to the caller's retired list, which is reserved up front so
/// the freeze never allocates. Past that bound the queue is released in place.
fn retire(retired: &mut Vec<Queue>, queue: Queue) {
    if retired.len() < retired.capacity() {
        retired.push(queue);
    }
}

struct Client {
    id: u32,
    rendered: AtomicBool,
    queue: Option<Queue>,
}

/// One client's queue, mapped and pre-faulted on control before the freeze that
/// publishes it. `None` when the client needs no new queue.
pub(super) struct PreparedClient {
    id: u32,
    queue: Option<Queue>,
}
impl PreparedClient {
    /// A client that needs no new queue.
    pub(super) fn none(id: u32) -> Self {
        Self { id, queue: None }
    }
    /// Holds a queue a frozen change detached, for release after the freeze.
    pub(super) fn detached(id: u32, queue: Option<Queue>) -> Self {
        Self { id, queue }
    }
    pub(super) fn id(&self) -> u32 {
        self.id
    }
    pub(super) fn into_queue(self) -> Option<Queue> {
        self.queue
    }
}

pub(super) struct ClientMix {
    clients: Vec<Client>,
    geometry: Geometry,
    revision: u64,
    clock: Arc<ClientPcmClockOwner>,
    /// Bytes every queue of this endpoint may map together.
    budget: usize,
}
impl Drop for ClientMix {
    fn drop(&mut self) {
        self.clock.retire();
    }
}

impl ClientMix {
    pub(super) fn new(geometry: Geometry) -> Option<Self> {
        let mut clients = Vec::new();
        clients.try_reserve_exact(MAX_CLIENTS).ok()?;
        Some(Self {
            clients,
            geometry,
            revision: 1,
            clock: Arc::new(ClientPcmClockOwner::new().ok()?),
            budget: MAX_PCM_BYTES,
        })
    }

    #[cfg(test)]
    pub(super) fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        self
    }

    fn limit(&self) -> usize {
        client_limit(self.budget, self.geometry.render_channels)
    }

    /// Whether one more queue of the current shape fits the budget. A geometry change
    /// can widen every queue, so this is checked against the current shape at each
    /// mapping and install, not only at registration.
    fn fits_another(&self) -> bool {
        self.clients
            .iter()
            .filter(|client| client.queue.is_some())
            .count()
            < self.limit()
    }

    fn has_queue(&self, id: u32) -> bool {
        self.clients
            .iter()
            .any(|client| client.id == id && client.queue.is_some())
    }

    /// Whether `id` lacks a queue the next IO start can use: it has none, or its ring
    /// has used every epoch. A writer callback in flight is not exhaustion.
    pub(super) fn needs_queue(&self, id: u32) -> bool {
        self.geometry.render_channels != 0
            && self
                .clients
                .iter()
                .find(|client| client.id == id)
                .is_none_or(|client| client.queue.as_ref().is_none_or(Queue::exhausted))
    }

    /// Maps and grants a queue without touching the published client set. Shared
    /// access is enough: the caller holds device control, so the set cannot change.
    pub(super) fn prepare(&self, id: u32) -> Option<PreparedClient> {
        if !self.has_queue(id) && !self.fits_another() {
            return None;
        }
        let queue = if self.geometry.render_channels == 0 {
            None
        } else {
            Some(Queue::with_clock(self.configuration(), id, Arc::clone(&self.clock)).ok()?)
        };
        Some(PreparedClient { id, queue })
    }

    fn admits(&self, queue: Option<&Queue>) -> bool {
        queue.is_none_or(|queue| queue.belongs_to(&self.clock))
    }

    /// Registers a client, with a queue when it may render at once. The reserved client
    /// vector never allocates here; a refused queue stays in `prepared`.
    pub(super) fn register(&mut self, prepared: &mut PreparedClient) -> bool {
        let id = prepared.id;
        if self.revision == u64::MAX
            || self.clients.len() >= self.limit()
            || self.clients.iter().any(|client| client.id == id)
            || !self.admits(prepared.queue.as_ref())
        {
            return false;
        }
        self.revision += 1;
        self.clock.publish_membership(self.revision);
        self.clients.push(Client {
            id,
            rendered: AtomicBool::new(false),
            queue: prepared.queue.take(),
        });
        true
    }

    /// Publishes a queue mapped for a registered client's IO start. Its previous queue,
    /// if any, is detached into `retired`. A refused queue stays in `prepared`.
    pub(super) fn install(
        &mut self,
        prepared: &mut PreparedClient,
        retired: &mut Vec<Queue>,
    ) -> bool {
        if prepared.queue.is_none() {
            return true;
        }
        if self.revision == u64::MAX
            || !self.admits(prepared.queue.as_ref())
            || (!self.has_queue(prepared.id) && !self.fits_another())
        {
            return false;
        }
        let Some(client) = self
            .clients
            .iter_mut()
            .find(|client| client.id == prepared.id)
        else {
            return false;
        };
        if let Some(previous) = std::mem::replace(&mut client.queue, prepared.queue.take()) {
            retire(retired, previous);
        }
        client.rendered.store(false, Ordering::Relaxed);
        self.revision += 1;
        self.clock.publish_membership(self.revision);
        true
    }

    /// Unpublishes one client and returns its queue for release after the freeze.
    pub(super) fn remove(&mut self, id: u32) -> Option<Queue> {
        let index = self.clients.iter().position(|client| client.id == id)?;
        let client = self.clients.remove(index);
        self.revision = self.revision.saturating_add(1);
        self.clock.publish_membership(self.revision);
        client.queue
    }

    /// Adopts a new device geometry without mapping anything: every queue of the old
    /// shape is detached into `retired`, and readers drop it with the next grant list.
    pub(super) fn reconfigure(&mut self, geometry: Geometry, retired: &mut Vec<Queue>) {
        let shaped = self.geometry.render_channels == geometry.render_channels
            && self.geometry.capture_channels == geometry.capture_channels;
        self.geometry = geometry;
        if shaped {
            return;
        }
        for client in &mut self.clients {
            if let Some(queue) = client.queue.take() {
                retire(retired, queue);
            }
        }
        self.revision = self.revision.saturating_add(1);
        self.clock.publish_membership(self.revision);
    }

    fn configuration(&self) -> EndpointTransportConfig {
        EndpointTransportConfig {
            endpoint_id: self.geometry.endpoint_id,
            capacity_frames: QUEUE_FRAMES,
            render_channels: self.geometry.render_channels,
            capture_channels: self.geometry.capture_channels,
        }
    }

    pub(super) fn publish_clock(
        &mut self,
        clock: &Clock,
        rearm: bool,
        retired: &mut Vec<Queue>,
    ) -> bool {
        self.publish_clock_after(clock, rearm, retired, || true)
    }

    /// Keeps remote readers fenced until client queues and native stream preparation
    /// succeed. `prepare` runs while the shared clock is fenced but before any queue
    /// changes; if it refuses, the previous timeline stays published and every queue is
    /// untouched. A rearm detaches each queue without a spare epoch into `retired`
    /// instead of failing; that client maps a fresh queue at its next IO start.
    pub(super) fn publish_clock_after(
        &mut self,
        clock: &Clock,
        rearm: bool,
        retired: &mut Vec<Queue>,
        prepare: impl FnOnce() -> bool,
    ) -> bool {
        if !crate::formats::SAMPLE_RATES.contains(&clock.rate) || self.revision == u64::MAX {
            return false;
        }
        let (epoch, anchor) = clock.transport_epoch();
        let Some(publication) = self.clock.begin(ClientPcmTimeline {
            epoch,
            anchor,
            rate: clock.rate,
            running: clock.running,
        }) else {
            return false;
        };
        if !prepare() {
            publication.abandon();
            return false;
        }
        let mut detached = false;
        for client in &mut self.clients {
            if rearm {
                client.rendered.store(false, Ordering::Relaxed);
                if client
                    .queue
                    .as_ref()
                    .is_some_and(|queue| !queue.can_rearm())
                    && let Some(queue) = client.queue.take()
                {
                    retire(retired, queue);
                    detached = true;
                }
            }
            if let Some(queue) = &mut client.queue {
                // Only a retired ring refuses here, and a rearm detached those above.
                let _ = publication.configure(queue, rearm);
            }
        }
        if detached {
            self.revision += 1;
            self.clock.publish_membership(self.revision);
        }
        publication.commit();
        true
    }

    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    pub(super) fn append_snapshot(&self, snapshot: &mut ClientPcmSnapshot) -> bool {
        self.clients.iter().all(|client| {
            client
                .queue
                .as_ref()
                .is_none_or(|queue| snapshot.push(queue))
        })
    }

    pub(super) fn render(
        &self,
        client: u32,
        frame: u64,
        samples: &[f32],
        frames: u32,
    ) -> IoOutcome {
        if frame
            .checked_add(u64::from(frames))
            .is_none_or(|end| end > (1 << 48) - 1)
        {
            return IoOutcome::default();
        }
        let Some(client) = self.clients.iter().find(|candidate| candidate.id == client) else {
            return IoOutcome::default();
        };
        if samples.len() != frames as usize * self.geometry.render_channels as usize
            || !client
                .queue
                .as_ref()
                .is_some_and(|queue| queue.write(frame, samples, frames))
        {
            return IoOutcome::default();
        }
        // Release publishes that this client has PCM, after the queue committed its frames.
        client.rendered.store(true, Ordering::Release);
        IoOutcome::completed(frames)
    }

    pub(super) fn capture(&self, frame: u64, output: &mut [f32], frames: u32) -> IoOutcome {
        output.fill(0.0);
        let channels = self.geometry.capture_channels as usize;
        if channels == 0
            || frames as usize > crate::formats::MAX_BUFFER_FRAMES
            || frame
                .checked_add(u64::from(frames))
                .is_none_or(|end| end > (1 << 48) - 1)
            || output.len() != frames as usize * channels
        {
            return IoOutcome::default();
        }
        // Acquire observes each client's first completed PCM publication. Lifecycle is frozen
        // by the device IO pin; no client or epoch can be retired during this mix.
        let mut sources = self
            .clients
            .iter()
            .filter(|client| client.rendered.load(Ordering::Acquire));
        let Some(first) = sources.next().and_then(|client| client.queue.as_ref()) else {
            return IoOutcome::completed(0);
        };
        let render_channels = self.geometry.render_channels as usize;
        if render_channels == channels && sources.clone().next().is_none() {
            return IoOutcome::completed(first.read(frame, output, frames));
        }
        let mut available = [false; crate::formats::MAX_BUFFER_FRAMES];
        let mut scratch = [0.0; COPY_FRAMES * MAX_CHANNELS];
        let mut coverage = [false; COPY_FRAMES];
        let mut mix = |queue: &Queue| {
            for (tile, destinations) in output.chunks_mut(COPY_FRAMES * channels).enumerate() {
                let offset = tile * COPY_FRAMES;
                let count = destinations.len() / channels;
                queue.read_with_coverage(
                    frame + offset as u64,
                    &mut scratch[..count * render_channels],
                    &mut coverage[..count],
                );
                for (index, (destination, samples)) in destinations
                    .chunks_exact_mut(channels)
                    .zip(scratch.chunks_exact(render_channels))
                    .enumerate()
                {
                    available[offset + index] |= coverage[index];
                    for (destination, sample) in destination.iter_mut().zip(samples) {
                        *destination += sample;
                    }
                }
            }
        };
        mix(first);
        for client in sources {
            if let Some(queue) = &client.queue {
                mix(queue);
            }
        }
        IoOutcome::completed(
            available[..frames as usize]
                .iter()
                .fold(0_u32, |count, valid| count + u32::from(*valid)),
        )
    }
}

#[cfg(test)]
mod tests;
