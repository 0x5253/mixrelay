//! Bounded clock-break history; callbacks publish, independent control cursors read.
use super::DeviceIo;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

/// Maximum unread events retained per native owner before overwrite is reported.
pub const CLOCK_BREAK_CAPACITY: usize = 64;
// Slot selection truncates positions to usize, which is exact only for a power of two.
const _: () = assert!(CLOCK_BREAK_CAPACITY.is_power_of_two());

/// Native positions at one detected timeline break; no captured PCM is retained.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockBreak {
    /// Cumulative native continuity counter, independent of the journal cursor.
    pub count: u64,
    /// Previous packet's sample position plus its actual frame count.
    pub expected_sample: f64,
    /// Sample position supplied by the discontinuous packet.
    pub actual_sample: f64,
    /// Prior packet's native host-clock ticks, not callback arrival time.
    pub previous_host: u64,
    /// Discontinuous packet's native host-clock ticks.
    pub actual_host: u64,
    /// Unmodified selected input/output timestamp-validity flags.
    pub flags: u64,
    /// `CoreAudio` `inNow` host ticks: IO-cycle start including wake-up scheduling latency.
    /// This is not time spent executing the Rust renderer.
    pub callback_host: u64,
    /// `inNow` validity flags; zero when no callback timestamp was supplied.
    pub callback_flags: u64,
    /// Actual host ticks at entry and completion of the preceding native packet's `IOProc`.
    /// Completion includes validation, transfer/rendering and credit publication.
    pub previous_enter: u64,
    pub previous_exit: u64,
    /// Actual entry ticks for this packet, distinct from `CoreAudio`'s `inNow` timestamp.
    pub callback_enter: u64,
}

/// One coherent event plus that reader's next cursor and overwrite accounting.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockBreakRecord {
    /// Position data from one callback publication, never mixed generations.
    pub event: ClockBreak,
    /// Feed this back as the next call's cursor for this same retained owner.
    pub next_cursor: u64,
    /// Events overwritten since that cursor, before this returned event.
    pub missed_before: u64,
}

#[derive(Default)]
struct Slot {
    sequence: AtomicU64,
    position: AtomicU64,
    fields: [AtomicU64; 11],
}

impl Slot {
    fn publish(&self, position: u64, event: ClockBreak) {
        self.sequence.fetch_add(1, SeqCst);
        self.position.store(position, SeqCst);
        let values = [
            event.count,
            event.expected_sample.to_bits(),
            event.actual_sample.to_bits(),
            event.previous_host,
            event.actual_host,
            event.flags,
            event.callback_host,
            event.callback_flags,
            event.previous_enter,
            event.previous_exit,
            event.callback_enter,
        ];
        for (field, value) in self.fields.iter().zip(values) {
            field.store(value, SeqCst);
        }
        self.sequence.fetch_add(1, SeqCst);
    }

    fn read(&self, position: u64) -> Option<ClockBreak> {
        let sequence = self.sequence.load(SeqCst);
        if sequence & 1 != 0 || self.position.load(SeqCst) != position {
            return None;
        }
        let values = self.fields.each_ref().map(|field| field.load(SeqCst));
        (sequence == self.sequence.load(SeqCst)).then_some(ClockBreak {
            count: values[0],
            expected_sample: f64::from_bits(values[1]),
            actual_sample: f64::from_bits(values[2]),
            previous_host: values[3],
            actual_host: values[4],
            flags: values[5],
            callback_host: values[6],
            callback_flags: values[7],
            previous_enter: values[8],
            previous_exit: values[9],
            callback_enter: values[10],
        })
    }
}

pub(super) struct ClockEvidence {
    published: AtomicU64,
    slots: [Slot; CLOCK_BREAK_CAPACITY],
}

impl Default for ClockEvidence {
    fn default() -> Self {
        Self {
            published: AtomicU64::new(0),
            slots: std::array::from_fn(|_| Slot::default()),
        }
    }
}

impl ClockEvidence {
    // DeviceIo's serialized IOProc is the sole writer, including reconnects.
    // Readers never advance or block this writer. Construction initializes every
    // atomic before publication; event recording performs no allocation or wait.
    pub(super) fn publish(&self, event: ClockBreak) {
        let position = self.published.load(SeqCst);
        self.slot(position).publish(position, event);
        self.published.store(position.wrapping_add(1), SeqCst);
    }

    fn read(&self, cursor: u64) -> Option<ClockBreakRecord> {
        let available = self.published.load(SeqCst).wrapping_sub(cursor);
        // Reject future or implausibly old cursors. Distances across integer wrap remain
        // valid; readers must poll before half the u64 publication space elapses.
        if available == 0 || available > u64::MAX / 2 {
            return None;
        }
        let missed_before = available.saturating_sub(CLOCK_BREAK_CAPACITY as u64);
        let position = cursor.wrapping_add(missed_before);
        let event = self.slot(position).read(position)?;
        Some(ClockBreakRecord {
            event,
            next_cursor: position.wrapping_add(1),
            missed_before,
        })
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "power-of-two capacity: truncation keeps the low position bits the modulus uses"
    )]
    fn slot(&self, position: u64) -> &Slot {
        &self.slots[position as usize % CLOCK_BREAK_CAPACITY]
    }
}

/// Reads one event after `cursor`, without consuming it for other readers.
/// Start at zero per owner, then use each successful record's `next_cursor`.
/// False leaves output untouched (empty, invalid cursor or concurrent overwrite);
/// retry on a later control tick, not in a realtime spin loop. Overwritten events
/// are counted in `missed_before` on the next successful read.
/// # Safety
/// `io` is null or initialized and retained throughout. A non-null `record`
/// owns aligned writable storage disjoint from the device. No callback-owned
/// mutable state is borrowed; only its atomic publication is accessed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_next_clock_break(
    io: *const DeviceIo,
    cursor: u64,
    record: *mut ClockBreakRecord,
) -> bool {
    // SAFETY: the native owner retains its initialized atomic journal.
    let Some(io) = (unsafe { io.as_ref() }) else {
        return false;
    };
    if record.is_null() {
        return false;
    }
    let Some(snapshot) = io.clock_evidence.read(cursor) else {
        return false;
    };
    // SAFETY: caller owns a complete, aligned output separate from retained IO.
    unsafe { record.write(snapshot) };
    true
}

const _: () = assert!(size_of::<ClockBreak>() == 88 && align_of::<ClockBreak>() == 8);
const _: () = assert!(size_of::<ClockBreakRecord>() == 104 && align_of::<ClockBreakRecord>() == 8);

#[cfg(test)]
mod tests;
