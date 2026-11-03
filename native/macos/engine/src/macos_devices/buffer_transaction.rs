//! Control-only native buffer transaction. The plan is validated in full before
//! mutation; failed readback rolls back in reverse order without overwriting a
//! newer setting made outside this transaction.
use std::collections::BTreeMap;

pub(super) trait BufferControl {
    fn current(&mut self, device: u32) -> u32;
    fn change(&mut self, device: u32, expected: u32, requested: u32) -> bool;
}

pub(super) struct BufferPlan {
    changes: BTreeMap<u32, u32>,
    requested: u32,
}

#[derive(Debug, PartialEq)]
pub(super) enum BufferFailure {
    InconsistentSnapshot,
    Restored,
    RollbackFailed,
}

impl BufferPlan {
    pub(super) fn prepare(
        observations: impl IntoIterator<Item = (u32, u32)>,
        requested: u32,
    ) -> Result<Self, BufferFailure> {
        let mut changes = BTreeMap::new();
        for (id, previous) in observations {
            if id == 0
                || previous == 0
                || requested == 0
                || changes
                    .insert(id, previous)
                    .is_some_and(|old| old != previous)
            {
                return Err(BufferFailure::InconsistentSnapshot);
            }
        }
        Ok(Self { changes, requested })
    }

    pub(super) fn apply(&self, control: &mut impl BufferControl) -> Result<(), BufferFailure> {
        let mut attempted = Vec::with_capacity(self.changes.len());
        for (&id, &previous) in &self.changes {
            if previous == self.requested {
                // A no-op snapshot is not proof that the device still has this
                // value. Reject a concurrent change without taking ownership
                // of it, and unwind only our earlier writes.
                if control.current(id) != self.requested {
                    return Err(self.rollback(control, &attempted));
                }
                continue;
            }
            // Even a failed setter may have changed the property before its
            // acknowledgement timed out, so include the failing device.
            attempted.push((id, previous));
            if !control.change(id, previous, self.requested) {
                return Err(self.rollback(control, &attempted));
            }
        }
        Ok(())
    }

    /// After successful application, a later native registration may still fail.
    /// Restore only changed values still owned by this plan, never no-op aliases.
    pub(super) fn restore(&self, control: &mut impl BufferControl) -> bool {
        let attempted = self
            .changes
            .iter()
            .filter(|(_, old)| **old != self.requested)
            .map(|(&id, &old)| (id, old))
            .collect::<Vec<_>>();
        self.rollback(control, &attempted) == BufferFailure::Restored
    }

    fn rollback(
        &self,
        control: &mut impl BufferControl,
        attempted: &[(u32, u32)],
    ) -> BufferFailure {
        let mut failed = false;
        for &(id, previous) in attempted.iter().rev() {
            let current = control.current(id);
            if current == self.requested {
                failed |= !control.change(id, current, previous);
            } else if current != previous {
                failed = true;
            }
        }
        if failed {
            BufferFailure::RollbackFailed
        } else {
            BufferFailure::Restored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Control {
        values: BTreeMap<u32, u32>,
        calls: Vec<(u32, u32, u32)>,
        failed_write: Option<(u32, u32)>,
        fail_rollback: bool,
    }
    impl BufferControl for Control {
        fn current(&mut self, id: u32) -> u32 {
            self.values[&id]
        }
        fn change(&mut self, id: u32, expected: u32, requested: u32) -> bool {
            self.calls.push((id, expected, requested));
            if self.values[&id] != expected {
                return false;
            }
            if requested == 32
                && let Some((failed, actual)) = self.failed_write
                && id == failed
            {
                self.values.insert(id, actual);
                return false;
            }
            if requested == 256 && self.fail_rollback {
                return false;
            }
            self.values.insert(id, requested);
            true
        }
    }
    fn fixture() -> (BufferPlan, Control) {
        let values = BTreeMap::from([(1, 256), (2, 256)]);
        (
            BufferPlan::prepare(values.clone(), 32).unwrap(),
            Control {
                values,
                ..Control::default()
            },
        )
    }
    #[test]
    fn shared_native_device_is_changed_once_and_conflicting_snapshots_are_rejected() {
        let (_, mut control) = fixture();
        BufferPlan::prepare([(1, 256), (1, 256)], 32)
            .unwrap()
            .apply(&mut control)
            .unwrap();
        assert_eq!(control.calls, [(1, 256, 32)]);
        assert!(matches!(
            BufferPlan::prepare([(1, 256), (1, 128)], 32),
            Err(BufferFailure::InconsistentSnapshot)
        ));
    }
    #[test]
    fn failed_readback_restores_every_attempted_device_in_reverse_order() {
        let (plan, mut control) = fixture();
        control.failed_write = Some((2, 32));
        assert_eq!(plan.apply(&mut control), Err(BufferFailure::Restored));
        assert_eq!(control.values, BTreeMap::from([(1, 256), (2, 256)]));
        assert_eq!(&control.calls[2..], [(2, 32, 256), (1, 32, 256)]);
    }
    #[test]
    fn rollback_preserves_external_changes_and_still_restores_other_devices() {
        let (plan, mut control) = fixture();
        control.failed_write = Some((2, 128));
        assert_eq!(plan.apply(&mut control), Err(BufferFailure::RollbackFailed));
        assert_eq!(control.values, BTreeMap::from([(1, 256), (2, 128)]));
        assert_eq!(control.calls.last(), Some(&(1, 32, 256)));
    }
    #[test]
    fn rollback_failure_is_reported_not_disguised_as_restoration() {
        let (plan, mut control) = fixture();
        control.failed_write = Some((2, 256));
        control.fail_rollback = true;
        assert_eq!(plan.apply(&mut control), Err(BufferFailure::RollbackFailed));
        assert_eq!(control.values[&1], 32);
    }
    #[test]
    fn no_op_buffers_do_not_acquire_rollback_ownership() {
        let (_, mut control) = fixture();
        BufferPlan::prepare([(1, 256)], 256)
            .unwrap()
            .apply(&mut control)
            .unwrap();
        assert!(control.calls.is_empty());
    }

    #[test]
    fn stale_no_op_rejects_application_and_preserves_external_value() {
        let plan = BufferPlan::prepare([(1, 256), (2, 32)], 32).unwrap();
        let mut control = Control {
            values: BTreeMap::from([(1, 256), (2, 128)]),
            ..Control::default()
        };
        assert_eq!(plan.apply(&mut control), Err(BufferFailure::Restored));
        assert_eq!(control.values, BTreeMap::from([(1, 256), (2, 128)]));
        assert_eq!(control.calls, [(1, 256, 32), (1, 32, 256)]);
    }

    #[test]
    fn later_connection_failure_restores_only_owned_changes_after_successful_apply() {
        let (plan, mut control) = fixture();
        plan.apply(&mut control).unwrap();
        assert!(plan.restore(&mut control));
        assert_eq!(control.values, BTreeMap::from([(1, 256), (2, 256)]));
        plan.apply(&mut control).unwrap();
        control.values.insert(2, 128); // A separate owner changed device 2.
        assert!(!plan.restore(&mut control));
        assert_eq!(control.values, BTreeMap::from([(1, 256), (2, 128)]));
    }
}
