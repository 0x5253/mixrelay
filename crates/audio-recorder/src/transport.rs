//! Bounded callback PCM transport and off-thread producer retirement coordination.

use crate::AdmissionFailure;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

mod packets;
pub(crate) use packets::Packet;
use packets::PacketRing;

pub(crate) const PACKET_FRAMES: usize = 256;
// Packet-rate tags allow a take to change input rate without reallocating its queue.
pub(crate) const PACKETS: usize =
    (mixrelay_audio_io::retention::FRAMES as usize).div_ceil(PACKET_FRAMES);
const MAX_CALLBACK_FRAMES: usize = 4096;
const FAULT_SHIFT: u32 = 56;
const CLOSED: u64 = 1 << 63;
pub(crate) const MAX_TOKEN: u64 = (1 << FAULT_SHIFT) - 1;

/// A cheap handle to a preallocated transport. Clone/Drop belong to render-plan
/// preparation/retirement, not an audio invocation. The registry owns the worker.
#[derive(Debug)]
pub struct RecorderTap {
    pub(crate) shared: Arc<Transport>,
}

#[derive(Debug)]
pub(crate) struct Transport {
    pub(crate) channels: u16,
    pub(crate) rate: AtomicU32,
    pub(crate) gate: AtomicU64,
    pub(crate) taps: AtomicUsize,
    producer: AtomicBool,
    in_flight: AtomicUsize,
    packets: PacketRing,
    // Only off-RT cutoff calls this hint while an admitted producer is still active.
    producer_wait: fn(),
}

impl Transport {
    pub(crate) fn new(
        channels: u16,
        producer_wait: fn(),
    ) -> Result<Arc<Self>, std::collections::TryReserveError> {
        Ok(Arc::new(Self {
            channels,
            rate: AtomicU32::new(0),
            gate: AtomicU64::new(0),
            taps: AtomicUsize::new(0),
            producer: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            packets: PacketRing::new(usize::from(channels))?,
            producer_wait,
        }))
    }

    /// Only the off-RT session owner calls this before draining commands.
    pub(crate) fn cutoff(&self) -> Option<AdmissionFailure> {
        // Preserve the token until its producer has left: a late invalid block
        // or overflow must remain a visible failure, even after admission closes.
        self.gate.fetch_or(CLOSED, Ordering::SeqCst);
        self.wait_producer();
        // Cutoff transfers the producer-owned partial slot to this control
        // thread. Preserve it for Stop/Pause and for admission-fault salvage.
        self.packets.finish_pending();
        fault(self.gate.load(Ordering::SeqCst))
    }

    pub(crate) fn wait_producer(&self) {
        while self.in_flight.load(Ordering::SeqCst) != 0 {
            (self.producer_wait)();
        }
    }

    pub(crate) fn acknowledge_failure(&self) {
        // The worker has closed admission, waited for every entrant (including
        // rejected overlapping producers), and copied the failure into status.
        self.gate.store(CLOSED, Ordering::SeqCst);
    }

    pub(crate) fn read_packet(&self, packet: &mut Packet) -> bool {
        self.packets.read(packet)
    }

    fn fail(&self, token: u64, code: u64) {
        let marked = token | (code << FAULT_SHIFT);
        if self
            .gate
            .compare_exchange(token, marked, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            let _ = self.gate.compare_exchange(
                token | CLOSED,
                marked | CLOSED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }
}

pub(crate) fn fault(gate: u64) -> Option<AdmissionFailure> {
    match (gate & !CLOSED) >> FAULT_SHIFT {
        0 => None,
        1 => Some(AdmissionFailure::Overrun),
        2 => Some(AdmissionFailure::InvalidPcm),
        _ => Some(AdmissionFailure::OverlappingProducers),
    }
}

struct Producer<'a>(&'a AtomicBool);
impl Drop for Producer<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

struct Flight<'a>(&'a AtomicUsize);
impl Drop for Flight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RecorderTap {
    pub(crate) fn new(shared: Arc<Transport>) -> Self {
        shared.taps.fetch_add(1, Ordering::SeqCst);
        Self { shared }
    }
    /// Capture one interleaved block, with its actual processing rate.
    ///
    /// This performs no allocation, lock, file I/O, wakeup or waiting. At most
    /// 4096 frames are copied into 256-frame packets (at most 17 slot claims
    /// including a carried partial packet). Invalid geometry, queue
    /// exhaustion or overlapping producers fails only this recorder. Calling
    /// while stopped/paused still observes the rate for the next Record command.
    pub fn capture(&self, samples: &[f32], sample_rate: u32) {
        let shared = &self.shared;
        let valid_rate = crate::valid_rate(sample_rate);
        if valid_rate {
            shared.rate.store(sample_rate, Ordering::Release);
        }
        let token = shared.gate.load(Ordering::SeqCst);
        if token == 0 || token > MAX_TOKEN {
            return;
        }
        shared.in_flight.fetch_add(1, Ordering::SeqCst);
        let _flight = Flight(&shared.in_flight);
        if shared.gate.load(Ordering::SeqCst) != token {
            return;
        }
        if shared
            .producer
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            shared.fail(token, 3);
            return;
        }
        let _producer = Producer(&shared.producer);
        // The monotonic admission token prevents a callback paused before Stop
        // from publishing into a later take or a later Resume.
        if shared.gate.load(Ordering::SeqCst) != token {
            return;
        }
        let channels = usize::from(shared.channels);
        if !valid_rate
            || !samples.len().is_multiple_of(channels)
            || samples.len() / channels > MAX_CALLBACK_FRAMES
        {
            shared.fail(token, 2);
            return;
        }
        for source in samples.chunks(PACKET_FRAMES * channels) {
            if !shared.packets.publish(source, sample_rate) {
                shared.fail(token, 1);
                return;
            }
        }
    }
}

impl Clone for RecorderTap {
    fn clone(&self) -> Self {
        Self::new(Arc::clone(&self.shared))
    }
}

impl Drop for RecorderTap {
    fn drop(&mut self) {
        self.shared.taps.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests;
