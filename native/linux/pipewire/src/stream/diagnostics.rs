//! Fixed native transfer observations, separate from stream and PCM ownership.
use super::{NativeStream, mr_pw_stream_clock, mr_pw_stream_transfer_snapshot};

/// One stream's lifetime diagnostic failures, in the C stage order.
/// Capture stage zero includes malformed ingress and ring admission failures;
/// unknown geometry increases calls with zero known frames. Ordinary dequeue-null
/// and declared empty capture chunks are not rejected writes.
#[cfg(any(test, feature = "test-observation"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[repr(C)]
pub struct RejectedTransfer {
    pub rejected_calls: u64,
    pub rejected_frames: u64,
    pub first_position: u64,
    pub last_position: u64,
    pub first_clock: u64,
    pub last_clock: u64,
}

/// Independent atomic observations; cursors and failures are not a transaction.
#[cfg(any(test, feature = "test-observation"))]
#[derive(Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[repr(C)]
pub struct NativeTransferSnapshot {
    pub read: u64,
    pub written: u64,
    pub queued: u64,
    /// Native capture write, native output read, engine capture read, engine output write.
    pub failures: [RejectedTransfer; 4],
    /// Successful engine delivery, distinct from the consumer cursor above.
    pub engine_read_frames: u64,
    /// Explicit consumer discard invocations, including empty queues.
    pub consumer_discard_calls: u64,
    /// Cursor advance inside explicit consumer discards.
    pub consumer_discard_frames: u64,
    /// Native state, format or clock changes that requested a ring epoch restart.
    pub native_restart_calls: u64,
    /// Failed output release operations, separate from PCM failures and clock transitions.
    pub native_output_trigger_rejections: u64,
}

#[cfg(any(test, feature = "test-observation"))]
impl std::fmt::Debug for NativeTransferSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeTransferSnapshot")
            .field("read", &self.read)
            .field("written", &self.written)
            .field("queued", &self.queued)
            .field("native_capture_write", &self.failures[0])
            .field("native_output_read", &self.failures[1])
            .field("engine_capture_read", &self.failures[2])
            .field("engine_output_write", &self.failures[3])
            .field("engine_read_frames", &self.engine_read_frames)
            .field("consumer_discard_calls", &self.consumer_discard_calls)
            .field("consumer_discard_frames", &self.consumer_discard_frames)
            .field("native_restart_calls", &self.native_restart_calls)
            .field(
                "native_output_trigger_rejections",
                &self.native_output_trigger_rejections,
            )
            .finish()
    }
}

/// Fixed owned-set totals. Positions/clock labels describe the first enumerated
/// stream only and can be joined to PCM tags only when `streams == 1`.
/// Lifetime counts and cursor sums wrap unsigned; compare with wrapping subtraction.
#[cfg(any(test, feature = "test-observation"))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TransferSnapshot {
    pub streams: u64,
    pub owner_sequence: u64,
    /// First owner's current atomic clock: ID high32, rate low32; zero unavailable.
    pub native_clock: u64,
    /// First owner's current graph frames high32, native PCM frames low32; zero unavailable.
    /// Independent of the clock and failure observations, not failure-time geometry.
    pub native_quantum: u64,
    pub counters: NativeTransferSnapshot,
}

#[cfg(any(test, feature = "test-observation"))]
/// Fixed test-owner publication size, including separately observed native clock geometry.
pub const TRANSFER_SNAPSHOT_WORDS: usize = 36;

#[cfg(any(test, feature = "test-observation"))]
impl TransferSnapshot {
    /// Folds a borrowed live stream; never changes its normal report baseline.
    pub fn include(&mut self, stream: &NativeStream) {
        if self.streams == 0 {
            self.owner_sequence = stream.allocation.observation_owner;
            // SAFETY: this borrowed owner keeps the stream live. Both getters
            // copy atomic observations and never borrow native callback IO.
            self.native_clock = unsafe { mr_pw_stream_clock(stream.allocation.pointer.as_ptr()) };
            self.native_quantum = stream.callback_quantum();
        }
        // SAFETY: the current set borrows its owning NativeStream. Its allocation
        // and native ring remain live; C reads atomics, never borrowed IO memory.
        self.include_counters(unsafe {
            mr_pw_stream_transfer_snapshot(stream.allocation.pointer.as_ptr())
        });
    }

    fn include_counters(&mut self, next: NativeTransferSnapshot) {
        if self.streams == 0 {
            self.counters = next;
        } else {
            self.counters.read = self.counters.read.wrapping_add(next.read);
            self.counters.written = self.counters.written.wrapping_add(next.written);
            self.counters.queued = self.counters.queued.wrapping_add(next.queued);
            self.counters.engine_read_frames = self
                .counters
                .engine_read_frames
                .wrapping_add(next.engine_read_frames);
            self.counters.consumer_discard_calls = self
                .counters
                .consumer_discard_calls
                .wrapping_add(next.consumer_discard_calls);
            self.counters.consumer_discard_frames = self
                .counters
                .consumer_discard_frames
                .wrapping_add(next.consumer_discard_frames);
            self.counters.native_restart_calls = self
                .counters
                .native_restart_calls
                .wrapping_add(next.native_restart_calls);
            self.counters.native_output_trigger_rejections = self
                .counters
                .native_output_trigger_rejections
                .wrapping_add(next.native_output_trigger_rejections);
            for (total, next) in self.counters.failures.iter_mut().zip(next.failures) {
                total.rejected_calls = total.rejected_calls.wrapping_add(next.rejected_calls);
                total.rejected_frames = total.rejected_frames.wrapping_add(next.rejected_frames);
            }
        }
        self.streams = self.streams.wrapping_add(1);
    }

    /// Packs only fixed integers for the test owner's control-readable atomics.
    #[must_use]
    pub fn words(self) -> [u64; TRANSFER_SNAPSHOT_WORDS] {
        std::array::from_fn(|index| match index {
            0 => self.streams,
            1 => self.counters.read,
            2 => self.counters.written,
            3 => self.counters.queued,
            28 => self.owner_sequence,
            29 => self.native_clock,
            30 => self.native_quantum,
            31 => self.counters.engine_read_frames,
            32 => self.counters.consumer_discard_calls,
            33 => self.counters.consumer_discard_frames,
            34 => self.counters.native_restart_calls,
            35 => self.counters.native_output_trigger_rejections,
            _ => {
                let failure = self.counters.failures[(index - 4) / 6];
                [
                    failure.rejected_calls,
                    failure.rejected_frames,
                    failure.first_position,
                    failure.last_position,
                    failure.first_clock,
                    failure.last_clock,
                ][(index - 4) % 6]
            }
        })
    }

    /// Reconstructs an independently loaded observation, not a coherent queue transaction.
    #[must_use]
    pub fn from_words(words: &[u64; TRANSFER_SNAPSHOT_WORDS]) -> Self {
        Self {
            streams: words[0],
            owner_sequence: words[28],
            native_clock: words[29],
            native_quantum: words[30],
            counters: NativeTransferSnapshot {
                read: words[1],
                written: words[2],
                queued: words[3],
                engine_read_frames: words[31],
                consumer_discard_calls: words[32],
                consumer_discard_frames: words[33],
                native_restart_calls: words[34],
                native_output_trigger_rejections: words[35],
                failures: std::array::from_fn(|index| {
                    let offset = 4 + index * 6;
                    RejectedTransfer {
                        rejected_calls: words[offset],
                        rejected_frames: words[offset + 1],
                        first_position: words[offset + 2],
                        last_position: words[offset + 3],
                        first_clock: words[offset + 4],
                        last_clock: words[offset + 5],
                    }
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests;
