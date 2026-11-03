//! Fixed storage between native callbacks and the non-real-time control worker.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::{
    Action, Binding, Kind, Mode,
    relative::{AtomicMovement, Movement},
};

// Every channel has 128 Note and 128 CC controls; no callback grows this storage.
const SLOTS: usize = 16 * 256;
// Retain the latest 1,024 discrete presses per port while dispatch is blocked.
const PRESS_CAPACITY: usize = 1024;
// Pack a queue cursor stamp beside the 38-bit receipt order and 12-bit control identity.
const PRESS_CURSOR_MASK: u64 = (1 << 14) - 1;
const DIRTY: u64 = 1 << 63;
const HIGH: u64 = 1 << 62;
const VALUE_MASK: u64 = 127;
const RETAIN: u64 = HIGH | VALUE_MASK;
// The raw word reserves 24 bits for value/presses and two for dirty/threshold flags.
const SEQUENCE_MASK: u64 = (1 << 38) - 1;
/// Owned by the resident worker and shared by its ports for cross-device ordering.
#[derive(Debug, Default)]
pub struct InputSequence(AtomicU64);

/// A callback's receipt stamp, reserved before inspecting map-mode state.
#[derive(Debug)]
pub struct Publication<'a> {
    mailbox: &'a Mailbox,
    sequence: u64,
    clearing: bool,
    generation: u64,
}

impl Publication<'_> {
    /// Publish using the cancellation generation observed when this callback began.
    pub fn receive(self, bytes: &[u8]) {
        if self.clearing {
            self.mailbox.suppress(bytes);
            return;
        }
        self.mailbox.receive_with_sequence(
            bytes,
            (self.sequence, self.generation),
            &mut |slot, old, next| {
                slot.compare_exchange(old, next, Ordering::SeqCst, Ordering::SeqCst)
            },
        );
    }
}

impl Drop for Publication<'_> {
    fn drop(&mut self) {
        // Release makes all callback payloads visible before the worker observes idle.
        self.mailbox.publishing.store(false, Ordering::Release);
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TakeStage {
    Checked,
    Consumed,
}

type CompareBatch<'a> = dyn FnMut(&AtomicU64, u64, u64) -> Result<u64, u64> + 'a;

/// One drained batch; relative movement accumulates while absolute values coalesce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub channel: u8,
    pub kind: Kind,
    pub number: u8,
    pub value: u8,
    pub presses: u16,
    sequence: u64,
    movements: [Movement; 8],
}

/// The adapter allocates this once per port, outside the native callback.
#[derive(Debug)]
pub struct Mailbox {
    slots: Box<[Slot]>,
    sequence: Arc<InputSequence>,
    generation: AtomicU64,
    clearing: AtomicBool,
    presses: Box<[StampedWord]>,
    next_press: AtomicU64,
    consumed_press: AtomicU64,
    publishing: AtomicBool,
}

#[derive(Debug)]
struct Slot {
    raw: StampedWord,
    movements: [AtomicMovement; 8],
}

/// One serial producer publishes generation and payload under a bounded snapshot protocol.
#[derive(Debug, Default)]
struct StampedWord {
    version: AtomicU64,
    generation: AtomicU64,
    value: AtomicU64,
}

struct Writing<'a>(&'a StampedWord, bool);

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.version.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl StampedWord {
    fn publish(&self, generation: u64) -> (Writing<'_>, bool) {
        // SeqCst orders both metadata and payload against the reader's two version
        // checks. Odd versions reject a partial publication without waiting.
        let reset = self.generation.load(Ordering::SeqCst) != generation;
        if reset {
            self.version.fetch_add(1, Ordering::SeqCst);
            self.generation.store(generation, Ordering::SeqCst);
        }
        (Writing(self, reset), reset)
    }

    fn read(&self) -> Option<(u64, u64)> {
        let version = self.version.load(Ordering::SeqCst);
        let generation = self.generation.load(Ordering::SeqCst);
        let value = self.value.load(Ordering::SeqCst);
        (version & 1 == 0 && self.version.load(Ordering::SeqCst) == version)
            .then_some((value, generation))
    }
}

impl Input {
    pub(crate) fn relative_gain(&self, offset: bool, step_db: f32, current: f32) -> Option<f32> {
        Some(self.movement(offset, step_db)?.apply(current))
    }

    fn movement(&self, offset: bool, step_db: f32) -> Option<Movement> {
        let steps: [f32; 4] = [0.5, 1.0, 2.0, 3.0];
        let step = steps
            .iter()
            .position(|step| step.to_bits() == step_db.to_bits())?;
        Some(self.movements[step + if offset { 4 } else { 0 }])
    }

    /// Global callback receipt order, independent of binding or device iteration order.
    #[must_use]
    pub fn sequence(&self, binding: &Binding) -> u64 {
        if binding.action == Action::Volume && binding.mode != Mode::Absolute {
            self.movement(binding.mode == Mode::Offset64, binding.step_db)
                .map_or(self.sequence, Movement::sequence)
        } else {
            self.sequence
        }
    }
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new(Arc::default())
    }
}

impl Mailbox {
    /// Allocate a port's fixed storage with the worker's shared receipt counter.
    #[must_use]
    pub fn new(sequence: Arc<InputSequence>) -> Self {
        Self {
            sequence,
            generation: AtomicU64::new(0),
            clearing: AtomicBool::new(false),
            presses: (0..PRESS_CAPACITY)
                .map(|_| StampedWord::default())
                .collect(),
            next_press: AtomicU64::new(0),
            consumed_press: AtomicU64::new(0),
            publishing: AtomicBool::new(false),
            slots: (0..SLOTS)
                .map(|_| Slot {
                    raw: StampedWord::default(),
                    movements: std::array::from_fn(|_| AtomicMovement::new()),
                })
                .collect(),
        }
    }
}

impl Mailbox {
    /// Ignore clock, `SysEx`, release velocity and unsupported/malformed messages.
    /// At most eight CAS attempts per aggregate: a callback never waits for the worker.
    pub fn receive(&self, bytes: &[u8]) {
        self.begin().receive(bytes);
    }

    /// Capture cancellation generation and reserve receipt order before inspecting learn state.
    /// The adapter serialises callbacks for each mailbox.
    #[must_use]
    pub fn begin(&self) -> Publication<'_> {
        // The receipt allocation publishes this in-flight marker. A worker that observes
        // the boundary can then defer a cross-port drain until every earlier input is ready.
        self.publishing.store(true, Ordering::Release);
        let generation = self.generation.load(Ordering::Acquire);
        let sequence = self.sequence.0.fetch_add(1, Ordering::AcqRel) & SEQUENCE_MASK;
        Publication {
            mailbox: self,
            sequence,
            // The final false acquires the completed cancellation generation update.
            clearing: self.clearing.load(Ordering::Acquire),
            generation,
        }
    }

    #[cfg(test)]
    fn receive_with(&self, bytes: &[u8], compare: &mut CompareBatch<'_>) {
        let publication = self.begin();
        self.receive_with_sequence(
            bytes,
            (publication.sequence, publication.generation),
            compare,
        );
    }

    fn receive_with_sequence(
        &self,
        bytes: &[u8],
        stamp: (u64, u64),
        compare: &mut CompareBatch<'_>,
    ) {
        let [status, number, value] = bytes else {
            return;
        };
        if *number > 127 || *value > 127 {
            return;
        }
        let (kind, value) = match status & 0xf0 {
            0x80 => (Kind::Note, 0),
            0x90 => (Kind::Note, *value),
            0xb0 => (Kind::Cc, *value),
            _ => return,
        };
        let channel = status & 15;
        let index = usize::from(channel) * 256
            + usize::from(*number)
            + if kind == Kind::Cc { 128 } else { 0 };
        let slot = &self.slots[index];
        let (sequence, generation) = stamp;
        let (_writing, reset) = slot.raw.publish(generation);
        if reset {
            slot.raw.value.fetch_and(RETAIN, Ordering::SeqCst);
        }
        if kind == Kind::Cc {
            for (index, movement) in slot.movements.iter().enumerate() {
                let step = [1, 2, 4, 6][index % 4];
                movement.append(relative(value, index >= 4) * step, sequence, reset);
            }
        }
        let slot = &slot.raw.value;
        // One atomic publishes the last absolute value, press count and receipt order.
        // SeqCst also orders generation transitions against the worker's snapshot.
        let mut old = slot.load(Ordering::Acquire);
        for _ in 0..8 {
            let press = value > 0 && (kind == Kind::Note || (value >= 64 && old & HIGH == 0));
            let pending = if reset { 0 } else { field(old, 8) };
            let presses = pending.saturating_add(u16::from(press));
            let next = DIRTY
                | u64::from(value)
                | if value >= 64 { HIGH } else { 0 }
                | (u64::from(presses) << 8)
                | (sequence << 24);
            match compare(slot, old, next) {
                Ok(_) => {
                    if press {
                        // midir serializes a port's FnMut callback. Publish the complete cell
                        // before advancing its tail; the consumer snapshots that boundary once.
                        let cursor = self.next_press.load(Ordering::Relaxed);
                        let cell = cursor % PRESS_CAPACITY as u64;
                        let cell = &self.presses[usize::try_from(cell).unwrap_or_default()];
                        let (writing, _) = cell.publish(generation);
                        cell.value.store(
                            ((cursor & PRESS_CURSOR_MASK) << 50) | (sequence << 12) | index as u64,
                            Ordering::SeqCst,
                        );
                        drop(writing);
                        self.next_press
                            .store(cursor.wrapping_add(1), Ordering::Release);
                    }
                    return;
                }
                Err(current) => old = current,
            }
        }
    }

    /// During learn, remember CC threshold changes without publishing actions.
    pub fn suppress(&self, bytes: &[u8]) {
        let [status, number, value] = bytes else {
            return;
        };
        if status & 0xf0 != 0xb0 || *number > 127 || *value > 127 {
            return;
        }
        let index = usize::from(status & 15) * 256 + usize::from(*number) + 128;
        self.slots[index].raw.value.store(
            u64::from(*value) | if *value >= 64 { HIGH } else { 0 },
            Ordering::Release,
        );
    }

    /// Discard historical input at a learn/reassignment boundary, preserving CC edges.
    pub fn clear(&self) {
        self.clear_with(|| {});
    }

    fn clear_with(&self, during: impl FnOnce()) {
        // Cancellation is independent of receipt order and compared only for equality.
        // A 64-bit generation cannot meaningfully wrap during a port's lifetime.
        self.clearing.store(true, Ordering::Release);
        during();
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.clearing.store(false, Ordering::Release);
    }

    /// Shared receipt boundary, captured once before inspecting any port's publication.
    #[must_use]
    pub fn boundary(&self) -> u64 {
        self.sequence.0.load(Ordering::Acquire) & SEQUENCE_MASK
    }

    /// Whether a callback that may precede that boundary is still publishing its payload.
    #[must_use]
    pub fn is_publishing(&self) -> bool {
        self.publishing.load(Ordering::Acquire)
    }

    /// Drain individual presses; callers merge ports using the shared receipt order.
    /// Releases and continuous values cannot move or overwrite a pending press.
    pub fn take_presses(&self) -> impl Iterator<Item = Input> + '_ {
        self.take_presses_before(self.boundary())
    }

    /// Drain only inputs before a shared cross-port boundary. Callers first check that
    /// every port is idle, so an earlier in-flight publication cannot arrive next batch.
    pub fn take_presses_before(&self, boundary: u64) -> impl Iterator<Item = Input> + '_ {
        // Acquire pairs with the producer tail. A fixed boundary excludes later presses,
        // even when the iterator is first advanced after a callback has published them.
        let end = self.next_press.load(Ordering::Acquire);
        let start = self
            .consumed_press
            .load(Ordering::Acquire)
            .max(end.saturating_sub(PRESS_CAPACITY as u64));
        (start..end).filter_map(move |cursor| {
            let cell = cursor % PRESS_CAPACITY as u64;
            let (raw, generation) =
                self.presses[usize::try_from(cell).unwrap_or_default()].read()?;
            // Skip overwritten cells and finish the captured prefix before other ports
            // dispatch later receipts. The replacement belongs to the next drain.
            if raw >> 50 != cursor & PRESS_CURSOR_MASK
                || self
                    .next_press
                    .load(Ordering::Acquire)
                    .saturating_sub(cursor)
                    > PRESS_CAPACITY as u64
            {
                return None;
            }
            if !reception_order((raw >> 12) & SEQUENCE_MASK, boundary).is_lt() {
                return None;
            }
            self.consumed_press.fetch_max(cursor + 1, Ordering::AcqRel);
            if generation != self.generation.load(Ordering::Acquire) {
                return None;
            }
            let index = raw & 4095;
            Some(Input {
                channel: u8::try_from(index / 256).unwrap_or_default(),
                kind: if index & 128 == 0 {
                    Kind::Note
                } else {
                    Kind::Cc
                },
                number: u8::try_from(index & 127).unwrap_or_default(),
                value: 127,
                presses: 1,
                sequence: (raw >> 12) & SEQUENCE_MASK,
                movements: std::array::from_fn(|_| Movement::identity()),
            })
        })
    }

    /// Consume one control without clearing the CC threshold history.
    #[must_use]
    pub fn take(&self, channel: u8, kind: Kind, number: u8) -> Option<Input> {
        self.take_with(channel, kind, number, &mut |_| {})
    }

    fn take_with(
        &self,
        channel: u8,
        kind: Kind,
        number: u8,
        checked: &mut dyn FnMut(TakeStage),
    ) -> Option<Input> {
        if channel > 15 || number > 127 {
            return None;
        }
        let index = usize::from(channel) * 256
            + usize::from(number)
            + if kind == Kind::Cc { 128 } else { 0 };
        let slot = &self.slots[index];
        let (old, generation) = slot.raw.read()?;
        if old & DIRTY == 0 || generation != self.generation.load(Ordering::Acquire) {
            return None;
        }
        checked(TakeStage::Checked);
        // A newer publication must retain its raw value for the next drain.
        slot.raw
            .value
            .compare_exchange(old, old & RETAIN, Ordering::SeqCst, Ordering::SeqCst)
            .ok()?;
        checked(TakeStage::Consumed);
        Some(Input {
            channel,
            kind,
            number,
            value: u8::try_from(old & VALUE_MASK).unwrap_or_default(),
            presses: field(old, 8),
            sequence: (old >> 24) & SEQUENCE_MASK,
            movements: std::array::from_fn(|index| {
                slot.movements[index].take((old >> 24) & SEQUENCE_MASK)
            }),
        })
    }
}

fn field(value: u64, shift: u32) -> u16 {
    u16::try_from((value >> shift) & 0xffff).unwrap_or_default()
}

/// Compare wrapped receipt counters; a drain window is much shorter than 2^37 inputs.
#[must_use]
pub fn reception_order(left: u64, right: u64) -> std::cmp::Ordering {
    let difference = left.wrapping_sub(right) & SEQUENCE_MASK;
    if difference == 0 {
        std::cmp::Ordering::Equal
    } else if difference <= SEQUENCE_MASK / 2 {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Less
    }
}

/// Common endless-encoder encodings, including their neutral values.
#[must_use]
pub fn relative(value: u8, offset: bool) -> i16 {
    if offset {
        i16::from(value) - 64
    } else if value < 64 {
        i16::from(value)
    } else {
        i16::from(value) - 128
    }
}

#[cfg(test)]
mod tests;
