//! Bounded, lock-free handoff from an LV2 UI callback to serialized plugin access.

use std::sync::atomic::{AtomicU32, Ordering};

// UI writes must be finite, so this NaN bit pattern cannot be a published value.
const EMPTY: u32 = f32::NAN.to_bits();

pub(super) struct PendingControls {
    input_offsets: Vec<usize>,
    slots: Vec<AtomicU32>,
}

impl PendingControls {
    pub(super) fn new(input_offsets: Vec<usize>, control_count: usize) -> Self {
        let slots = (0..control_count).map(|_| AtomicU32::new(EMPTY)).collect();
        Self {
            input_offsets,
            slots,
        }
    }

    pub(super) fn publish(&self, port_index: usize, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let Some(&offset) = self.input_offsets.get(port_index) else {
            return false;
        };
        let Some(slot) = self.slots.get(offset) else {
            return false;
        };
        slot.store(value.to_bits(), Ordering::Release);
        true
    }

    pub(super) fn take(&self, offset: usize) -> Option<f32> {
        let slot = self.slots.get(offset)?;
        // Skip the RMW on idle ports. A publish after this load stays pending.
        if slot.load(Ordering::Acquire) == EMPTY {
            return None;
        }
        let bits = slot.swap(EMPTY, Ordering::AcqRel);
        (bits != EMPTY).then(|| f32::from_bits(bits))
    }

    pub(super) fn peek(&self, offset: usize) -> Option<f32> {
        let bits = self.slots.get(offset)?.load(Ordering::Acquire);
        (bits != EMPTY).then(|| f32::from_bits(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn accepts_only_finite_input_controls() {
        let controls = PendingControls::new(vec![0, usize::MAX, 1], 2);
        assert!(!controls.publish(1, 0.5));
        assert!(!controls.publish(3, 0.5));
        assert!(!controls.publish(0, f32::NAN));
        assert!(!controls.publish(0, f32::INFINITY));
        assert_eq!(controls.peek(0), None);
        assert!(controls.publish(0, 0.25));
        assert!(controls.publish(0, 0.75));
        assert_eq!(controls.peek(0).map(f32::to_bits), Some(0.75_f32.to_bits()));
        assert_eq!(controls.take(0).map(f32::to_bits), Some(0.75_f32.to_bits()));
        assert_eq!(controls.take(0), None);
    }

    #[test]
    fn concurrent_publish_is_consumed_or_left_for_next_drain() {
        let controls = PendingControls::new(vec![0], 1);
        let finished = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for value in 1_u16..=10_000 {
                    assert!(controls.publish(0, f32::from(value)));
                }
                finished.store(true, Ordering::Release);
            });
            let mut latest = 0.0;
            while !finished.load(Ordering::Acquire) {
                if let Some(value) = controls.take(0) {
                    latest = value;
                }
            }
            if let Some(value) = controls.take(0) {
                latest = value;
            }
            assert_eq!(latest.to_bits(), 10_000.0_f32.to_bits());
        });
    }

    #[test]
    fn restore_boundary_keeps_writes_published_after_the_prior_drain() {
        let controls = PendingControls::new(vec![0], 1);
        assert!(controls.publish(0, 0.25));
        let connected = controls.take(0).expect("prior UI write should drain");
        assert_eq!(connected.to_bits(), 0.25_f32.to_bits());

        // A publish inside native restore is later than the pre-restore drain.
        assert!(controls.publish(0, 0.75));
        let restored = 0.5_f32;
        assert_eq!(controls.take(0).map(f32::to_bits), Some(0.75_f32.to_bits()));
        assert_eq!(restored.to_bits(), 0.5_f32.to_bits());

        // Failure must not clear a publication made after that same boundary.
        assert!(controls.publish(0, 0.125));
        assert_eq!(
            controls.take(0).map(f32::to_bits),
            Some(0.125_f32.to_bits())
        );
    }
}
