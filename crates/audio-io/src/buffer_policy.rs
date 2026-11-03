//! Queue targets in the queue's own frame units, independent of native APIs.
//! The queue owner samples actual callback geometry; this policy does not prime,
//! discard, allocate, choose a clock, or perform sample-rate conversion.

use crate::clock_bridge::ClockRelation;
use thiserror::Error;

/// Playback latency target ceiling, independent of retention storage: half of the
/// 16,384-frame device queue that first bounded it, so two 4,096-frame bursts fit.
/// Raising it lengthens worst-case playback latency and every queue sized from it.
pub const MAXIMUM_TARGET_FRAMES: usize = 8192;

/// Storage and minimum-burst constraints supplied by a prepared queue owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueBounds {
    /// Smallest callback burst to assume, including before the first callback.
    pub minimum_frames: u32,
    /// Physical storage, not a latency target. Half remains overflow headroom.
    pub capacity_frames: usize,
    /// Explicit target ceiling, independent of additional retention storage.
    pub maximum_target_frames: usize,
}

/// Invalid off-realtime queue policy configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum QueuePolicyError {
    /// Both sides need a nonzero nominal rate to convert frame units.
    #[error("queue and peer sample rates must be nonzero")]
    ZeroRate,
    /// The queue must fit its minimum burst and the same amount of headroom.
    #[error("queue capacity must hold at least twice its nonzero minimum burst")]
    InvalidBounds,
}

/// Prepared burst policy for one side of a clock boundary.
///
/// Construct once per format/clock change off RT. One queue's policy may be
/// copied or borrowed across threads; it contains no mutable state or handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueTargetPolicy {
    queue_rate: u32,
    peer_rate: u32,
    minimum_frames: u32,
    maximum_frames: usize,
    bursts: usize,
}

impl QueueTargetPolicy {
    /// Prepare rate conversion and limits for a queue and its peer callback.
    /// `relation` must describe actual clock ownership, not matching nominal Hz.
    /// Independent clocks retain two complete bursts; proven alignment needs one.
    ///
    /// # Errors
    /// Returns [`QueuePolicyError`] for zero rates or insufficient/zero bounds.
    ///
    /// # Examples
    /// ```
    /// use mixrelay_audio_io::{buffer_policy::{QueueBounds, QueueTargetPolicy},
    ///     clock_bridge::ClockRelation};
    /// let queue = QueueTargetPolicy::new(48_000, 44_100, ClockRelation::Independent,
    ///     QueueBounds { minimum_frames: 32, capacity_frames: 16_384, maximum_target_frames: 8192 })?;
    /// // The peer's 128 frames round up to 140 here; reserve two full bursts.
    /// assert_eq!(queue.target_frames(32, 128, 1), 280);
    /// # Ok::<(), mixrelay_audio_io::buffer_policy::QueuePolicyError>(())
    /// ```
    pub fn new(
        queue_rate: u32,
        peer_rate: u32,
        relation: ClockRelation,
        bounds: QueueBounds,
    ) -> Result<Self, QueuePolicyError> {
        if queue_rate == 0 || peer_rate == 0 {
            return Err(QueuePolicyError::ZeroRate);
        }
        if bounds.minimum_frames == 0
            || bounds.minimum_frames as usize > bounds.maximum_target_frames
            || bounds.maximum_target_frames > bounds.capacity_frames / 2
        {
            return Err(QueuePolicyError::InvalidBounds);
        }
        Ok(Self {
            queue_rate,
            peer_rate,
            minimum_frames: bounds.minimum_frames,
            maximum_frames: bounds.maximum_target_frames,
            bursts: match relation {
                ClockRelation::Independent => 2,
                ClockRelation::ProvenSameClock => 1,
            },
        })
    }

    /// Return the bounded target in this queue's frames.
    ///
    /// `queue_quantum` is the local callback burst; `peer_quantum` is in the
    /// peer's rate and rounds up on conversion. Both are floored to the configured
    /// minimum. `converter_quantum` is the actual converter chunk on this side,
    /// not its scratch capacity; use one for a direct same-clock copy.
    /// The complete largest burst must fit before applying independent-clock
    /// reserve. Capacity limits the result but never establishes the target.
    #[must_use]
    pub fn target_frames(
        &self,
        queue_quantum: u32,
        peer_quantum: u32,
        converter_quantum: usize,
    ) -> usize {
        // Widen before multiplication: native callback fields can span all u32
        // values, including on a 32-bit host. div_ceil does not add to the product.
        let mapped = (u64::from(peer_quantum.max(self.minimum_frames))
            * u64::from(self.queue_rate))
        .div_ceil(u64::from(self.peer_rate));
        let burst = (queue_quantum.max(self.minimum_frames) as usize)
            .max(usize::try_from(mapped).unwrap_or(usize::MAX))
            .max(converter_quantum);
        burst
            .saturating_mul(self.bursts)
            .clamp(self.minimum_frames as usize, self.maximum_frames)
    }
}

#[cfg(test)]
mod tests;
