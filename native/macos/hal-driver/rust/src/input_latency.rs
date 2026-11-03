//! Control publishes identity/timing; render callbacks only mark fixed atomic client slots.
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};

use crate::device::lock;

const CLIENT_SLOTS: usize = 256;
/// Reads of one publication before giving up. A publish holds the sequence odd only for
/// four stores, so a reader that overlaps more than this many is on a stalled publisher
/// and reports no latency (0) rather than spin on an IO thread.
const READ_ATTEMPTS: usize = 4;
#[derive(Default)]
struct Client {
    id: AtomicU64,
    pid: AtomicU32,
    /// Process start (see `ProcessInspection::process_start`); 0 when unresolved.
    start: AtomicU64,
    active: AtomicBool,
    rendered: AtomicBool,
}
/// One engine publication, read as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Publication {
    rate: u32,
    /// The publishing engine's PID; 0 when no latency is published.
    pid: u32,
    /// The publishing engine's process start; 0 when unresolved.
    start: u64,
    frames: u32,
}
pub(crate) struct InputLatency {
    clients: [Client; CLIENT_SLOTS],
    overflow: AtomicU32,
    /// Seqlock over the publication fields below: odd while a publish rewrites them.
    sequence: AtomicU64,
    /// Serializes publishers, which run on control paths; readers never take it.
    publisher: Mutex<()>,
    rate: AtomicU32,
    pid: AtomicU32,
    engine_start: AtomicU64,
    frames: AtomicU32,
    notified: AtomicU32,
}
impl Default for InputLatency {
    fn default() -> Self {
        Self {
            clients: std::array::from_fn(|_| Client::default()),
            overflow: AtomicU32::default(),
            sequence: AtomicU64::default(),
            publisher: Mutex::default(),
            rate: AtomicU32::default(),
            pid: AtomicU32::default(),
            engine_start: AtomicU64::default(),
            frames: AtomicU32::default(),
            notified: AtomicU32::default(),
        }
    }
}
impl InputLatency {
    fn client(&self, id: u32) -> Option<&Client> {
        self.clients
            .iter()
            .find(|client| client.id.load(Ordering::Acquire) == u64::from(id) + 1)
    }
    /// Registration/removal are serialized by the device control owner.
    pub(crate) fn add(&self, id: u32) {
        if let Some(client) = self
            .clients
            .iter()
            .find(|client| client.id.load(Ordering::Acquire) == 0)
        {
            client.pid.store(0, Ordering::Release);
            client.start.store(0, Ordering::Release);
            client.active.store(false, Ordering::Release);
            client.rendered.store(false, Ordering::Release);
            client.id.store(u64::from(id) + 1, Ordering::Release);
        } else {
            self.overflow.fetch_add(1, Ordering::AcqRel);
        }
    }
    pub(crate) fn identity(&self, id: u32, pid: u32, start: Option<u64>) {
        if let Some(client) = self.client(id) {
            client.start.store(start.unwrap_or(0), Ordering::Release);
            client.pid.store(pid, Ordering::Release);
        }
    }
    pub(crate) fn remove(&self, id: u32) {
        if let Some(client) = self.client(id) {
            client.active.store(false, Ordering::Release);
            client.rendered.store(false, Ordering::Release);
            client.id.store(0, Ordering::Release);
        } else {
            self.overflow.fetch_sub(1, Ordering::AcqRel);
        }
    }
    pub(crate) fn active(&self, id: u32, active: bool) {
        if let Some(client) = self.client(id) {
            client.active.store(active, Ordering::Release);
            if !active {
                client.rendered.store(false, Ordering::Release);
            }
        }
    }
    /// IO only: no registry/control/identity locks, allocation or host callbacks.
    pub(crate) fn render(&self, id: u32) {
        if let Some(client) = self.client(id)
            && client.active.load(Ordering::Acquire)
        {
            client.rendered.store(true, Ordering::Release);
        }
    }
    /// `start` is the engine's process start, resolved while it publishes. See
    /// `is_engine` for how a rendering client is matched to it. Control paths only.
    pub(crate) fn publish(&self, rate: u32, pid: u32, start: Option<u64>, frames: Option<u32>) {
        let _publisher = lock(&self.publisher);
        // Relaxed: only publishers write the sequence, and they are serialized above.
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        // Release fence: a reader that observes any field store below also observes the
        // odd sequence, so its closing sequence check fails.
        fence(Ordering::Release);
        // Relaxed: the fields are ordered by the sequence stores and fences.
        self.rate.store(rate, Ordering::Relaxed);
        self.pid.store(frames.map_or(0, |_| pid), Ordering::Relaxed);
        self.engine_start
            .store(start.unwrap_or(0), Ordering::Relaxed);
        self.frames.store(frames.unwrap_or(0), Ordering::Relaxed);
        // Release: a reader that observes the even sequence observes every field above.
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }
    /// The current publication, or `None` when every attempt overlapped a publish.
    /// `between` runs between field reads; production passes a no-op.
    fn publication(&self, mut between: impl FnMut()) -> Option<Publication> {
        for _ in 0..READ_ATTEMPTS {
            // Acquire: pairs with the publisher's closing Release store.
            let before = self.sequence.load(Ordering::Acquire);
            if before.is_multiple_of(2) {
                // Relaxed: validated by the sequence check below.
                let pid = self.pid.load(Ordering::Relaxed);
                let frames = self.frames.load(Ordering::Relaxed);
                between();
                let publication = Publication {
                    rate: self.rate.load(Ordering::Relaxed),
                    pid,
                    start: self.engine_start.load(Ordering::Relaxed),
                    frames,
                };
                // Acquire fence: the field loads above complete before the sequence
                // re-read, so any overlapping publish changes the sequence seen here.
                fence(Ordering::Acquire);
                if self.sequence.load(Ordering::Relaxed) == before {
                    return Some(publication);
                }
            }
            std::hint::spin_loop();
        }
        None
    }
    pub(crate) fn effective(&self, rate: u32) -> u32 {
        self.effective_with(rate, || {})
    }
    fn effective_with(&self, rate: u32, between: impl FnMut()) -> u32 {
        let Some(publication) = self.publication(between) else {
            return 0;
        };
        if publication.pid == 0
            || publication.rate != rate
            || self.overflow.load(Ordering::Acquire) != 0
            || self.clients.iter().any(|client| {
                client.active.load(Ordering::Acquire)
                    && client.rendered.load(Ordering::Acquire)
                    && !Self::is_engine(client, publication.pid, publication.start)
            })
        {
            return 0;
        }
        publication.frames
    }
    /// A rendering client is the engine when coreaudiod registered it for the engine's PID.
    /// When both process starts are known they must also agree, so a reused PID is not the
    /// engine. The driver sandbox denies `proc_pidinfo` for other processes, so starts are
    /// usually unknown there; then the registration's PID decides, as for transport type.
    /// A registration ends with its process, and the publication ends with the engine's
    /// lease, so a reused PID could only match within the lease expiry after an engine crash.
    fn is_engine(client: &Client, pid: u32, start: u64) -> bool {
        let client_start = client.start.load(Ordering::Acquire);
        client.pid.load(Ordering::Acquire) == pid
            && (start == 0 || client_start == 0 || client_start == start)
    }
    /// Control ticks coalesce IO-side gate changes; host notification happens outside locks.
    pub(crate) fn changed(&self, rate: u32) -> bool {
        let effective = self.effective(rate);
        self.notified.swap(effective, Ordering::AcqRel) != effective
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_only_gate_tracks_rendering_not_capture_and_recovers_on_stop() {
        let latency = InputLatency::default();
        latency.add(1);
        latency.identity(1, 10, Some(7));
        latency.active(1, true);
        latency.add(2);
        latency.identity(2, 20, Some(8));
        latency.active(2, true);
        latency.publish(48_000, 10, Some(7), Some(300));
        assert_eq!(latency.effective(48_000), 300);
        assert!(latency.changed(48_000));
        assert!(!latency.changed(48_000));
        latency.render(1);
        assert_eq!(latency.effective(48_000), 300);
        latency.render(2);
        assert_eq!(latency.effective(48_000), 0);
        assert!(latency.changed(48_000));
        latency.active(2, false);
        assert_eq!(latency.effective(48_000), 300);
        assert_eq!(latency.effective(44_100), 0);
        latency.remove(2);
        latency.remove(1);
        latency.render(2);
        latency.identity(2, 20, Some(8));
        latency.active(2, true);
        latency.publish(48_000, 10, Some(7), None);
        assert_eq!(latency.effective(48_000), 0);
    }
    #[test]
    fn a_reused_engine_pid_is_not_the_engine_and_unknown_starts_use_the_registration() {
        let latency = InputLatency::default();
        latency.add(1);
        latency.active(1, true);
        latency.render(1);
        for (pid, client, engine, expected) in [
            (10, Some(7), Some(7), 300),
            // Known starts that differ: the PID was reused.
            (10, Some(5), Some(7), 0),
            // The driver sandbox hides either start: coreaudiod's PID decides.
            (10, None, Some(7), 300),
            (10, Some(7), None, 300),
            (10, None, None, 300),
            // Another process rendering is never the engine.
            (20, None, None, 0),
        ] {
            latency.identity(1, pid, client);
            latency.publish(48_000, 10, engine, Some(300));
            assert_eq!(
                latency.effective(48_000),
                expected,
                "{pid} {client:?} {engine:?}"
            );
        }
    }
    #[test]
    fn a_publish_between_field_reads_never_mixes_old_frames_with_the_new_rate() {
        let latency = InputLatency::default();
        latency.publish(48_000, 10, Some(7), Some(300));
        // The reader has the old frames when the device moves to 44.1 kHz with new frames.
        let mut publish = Some(|| latency.publish(44_100, 10, Some(7), Some(500)));
        let effective = latency.effective_with(44_100, || {
            if let Some(publish) = publish.take() {
                publish();
            }
        });
        assert_eq!(effective, 500);
        // A rate change that clears the frames is never read with the old frames either.
        latency.publish(48_000, 10, Some(7), Some(300));
        let mut publish = Some(|| latency.publish(44_100, 10, Some(7), None));
        let effective = latency.effective_with(44_100, || {
            if let Some(publish) = publish.take() {
                publish();
            }
        });
        assert_eq!(effective, 0);
    }
    #[test]
    fn a_reader_overlapping_every_attempt_gives_up_with_no_latency() {
        let latency = InputLatency::default();
        latency.publish(48_000, 10, Some(7), Some(300));
        let mut attempts = 0;
        let effective = latency.effective_with(48_000, || {
            attempts += 1;
            latency.publish(48_000, 10, Some(7), Some(300));
        });
        assert_eq!((effective, attempts), (0, READ_ATTEMPTS));
        assert_eq!(latency.effective(48_000), 300);
    }
    #[test]
    fn untracked_or_unidentified_clients_fail_closed() {
        let latency = InputLatency::default();
        latency.publish(48_000, 10, Some(7), Some(300));
        for id in 0..u32::try_from(CLIENT_SLOTS).unwrap() {
            latency.add(id);
        }
        latency.add(u32::MAX);
        assert_eq!(latency.effective(48_000), 0);
        latency.remove(u32::MAX);
        assert_eq!(latency.effective(48_000), 300);
        latency.active(0, true);
        latency.render(0);
        assert_eq!(latency.effective(48_000), 0);
        latency.remove(0);
        assert_eq!(latency.effective(48_000), 300);
    }
}
