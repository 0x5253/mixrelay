use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

use mixrelay_audio_graph::{ExecutionPlan, NodeConfig, NodeControl, NodeId};
use parking_lot::Mutex;
use thiserror::Error;

/// A complete graph-local control value, published without rebuilding its processor.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeControlUpdate {
    /// Stable compiled node identity, including any graph-local projection prefix.
    pub node_id: NodeId,
    /// Execution gate and signal gate. OFF is not bypass.
    pub control: NodeControl,
    /// Replacement gain for a gain-capable node; `None` preserves its current gain.
    pub gain_db: Option<f32>,
}

/// A rejected control publication leaves every node in the batch unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeControlError {
    /// The target does not belong to this prepared topology.
    #[error("runtime node {0} does not exist")]
    UnknownNode(NodeId),
    /// Initially inactive providers must be prepared before their first activation.
    #[error("runtime node {0} needs preparation before it can be enabled")]
    NeedsPreparation(NodeId),
    /// Only gain kernels and native capture source kernels accept gain changes.
    #[error("runtime node {0} does not support gain")]
    UnsupportedGain(NodeId),
    /// Non-finite decibels or their non-finite linear conversion are rejected.
    #[error("runtime node {0} has an invalid gain")]
    InvalidGain(NodeId),
    /// Revisions must advance monotonically within a prepared topology.
    #[error("control revision {proposed} is not newer than {current}")]
    StaleRevision { current: u64, proposed: u64 },
    /// A handle from a removed/replaced topology must not command its old processor.
    #[error("control topology {proposed} does not match submitted topology {current}")]
    StalePlan { current: u64, proposed: u64 },
    /// A bank cannot be attached to two unrelated processor exchanges.
    #[error("runtime controls belong to another processor exchange")]
    DifferentExchange,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ControlValue {
    pub control: NodeControl,
    pub linear_gain: f32,
}

impl ControlValue {
    fn encode(self) -> u64 {
        u64::from(self.linear_gain.to_bits())
            | (u64::from(self.control.enabled) << 32)
            | (u64::from(self.control.muted) << 33)
    }

    fn decode(bits: u64) -> Self {
        Self {
            control: NodeControl {
                enabled: bits & (1 << 32) != 0,
                muted: bits & (1 << 33) != 0,
            },
            linear_gain: f32::from_bits(low_gain_bits(bits)),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the encoded gain occupies exactly the low 32 bits; truncation selects them"
)]
const fn low_gain_bits(bits: u64) -> u32 {
    bits as u32
}

#[derive(Debug)]
struct ControlCell {
    value: AtomicU64,
    supports_gain: bool,
    prepared: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ControlRevisionClock {
    pub published: AtomicU64,
    pub applied: AtomicU64,
}

/// Control-plane publisher and allocation-free audio-block snapshot source.
///
/// Cells retain compiled node order; a separate sorted index resolves stable identities off the
/// audio thread. Writers serialize here, but the audio reader never takes that mutex or retries.
/// An odd epoch or concurrent publication simply defers the complete batch until another block.
#[derive(Debug)]
pub struct RuntimeControlBank {
    plan_revision: u64,
    identities: Box<[(NodeId, usize)]>,
    cells: Box<[ControlCell]>,
    initial: Box<[ControlValue]>,
    writer: Mutex<()>,
    epoch: AtomicU64,
    published: AtomicU64,
    applied: AtomicU64,
    clock: OnceLock<Arc<ControlRevisionClock>>,
}

impl RuntimeControlBank {
    pub(crate) fn new(execution: &ExecutionPlan, prepared: &[bool]) -> Self {
        let mut identities: Vec<_> = execution
            .nodes()
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id().clone(), index))
            .collect();
        identities.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let cells: Box<[_]> = execution
            .nodes()
            .iter()
            .zip(prepared)
            .map(|(node, prepared)| {
                let gain_db = match node.config() {
                    NodeConfig::Gain(config) => Some(config.gain_db),
                    NodeConfig::AudioDeviceInput(config) => Some(config.gain_db),
                    NodeConfig::ApplicationCapture(config) => Some(config.gain_db),
                    _ => None,
                };
                ControlCell {
                    value: AtomicU64::new(
                        ControlValue {
                            control: node.control(),
                            linear_gain: gain_db.map_or(1.0, crate::dsp::db_to_linear),
                        }
                        .encode(),
                    ),
                    supports_gain: gain_db.is_some(),
                    prepared: *prepared,
                }
            })
            .collect();
        let initial = cells
            .iter()
            .map(|cell| ControlValue::decode(cell.value.load(Ordering::SeqCst)))
            .collect();
        Self {
            plan_revision: execution.revision(),
            identities: identities.into_boxed_slice(),
            cells,
            initial,
            writer: Mutex::new(()),
            epoch: AtomicU64::new(0),
            published: AtomicU64::new(execution.revision()),
            applied: AtomicU64::new(0),
            clock: OnceLock::new(),
        }
    }

    /// Topology revision that owns these stable runtime identities.
    #[must_use]
    pub const fn plan_revision(&self) -> u64 {
        self.plan_revision
    }

    /// Most recent complete control batch accepted by the control thread.
    #[must_use]
    pub fn published_revision(&self) -> u64 {
        self.published.load(Ordering::SeqCst)
    }

    /// Most recent complete batch sampled at an audio block boundary.
    #[must_use]
    pub fn applied_revision(&self) -> u64 {
        self.applied.load(Ordering::Acquire)
    }

    /// Reads one node's coherent gate/gain value without entering graph mutation machinery.
    /// The returned gain is linear; `None` means this topology does not contain the identity.
    #[must_use]
    pub fn snapshot(&self, node_id: &NodeId) -> Option<(NodeControl, f32)> {
        let value = self.value(self.index(node_id)?);
        Some((value.control, value.linear_gain))
    }

    /// Validates an entire proposed batch without changing any runtime values.
    ///
    /// # Errors
    /// Returns a target, preparation, gain, or stale revision error for an invalid batch.
    pub fn validate(
        &self,
        revision: u64,
        updates: &[RuntimeControlUpdate],
    ) -> Result<(), RuntimeControlError> {
        self.check_revision(revision)?;
        for update in updates {
            self.resolve(update)?;
        }
        Ok(())
    }

    fn check_revision(&self, revision: u64) -> Result<(), RuntimeControlError> {
        let current = self.published_revision();
        if revision <= current {
            return Err(RuntimeControlError::StaleRevision {
                current,
                proposed: revision,
            });
        }
        Ok(())
    }

    pub(crate) fn publish(
        &self,
        revision: u64,
        updates: &[RuntimeControlUpdate],
    ) -> Result<(), RuntimeControlError> {
        let _writer = self.writer.lock();
        // Validate the complete batch before the epoch opens so a rejected update leaves
        // every node unchanged and the audio reader never sees an unfinished publication.
        self.validate(revision, updates)?;
        // All payload accesses participate in the same total order as the epoch. This is
        // intentionally stronger than a relaxed seqlock: ARM cannot move the final epoch read
        // ahead of a payload load or expose a new payload under the preceding even epoch.
        self.epoch.fetch_add(1, Ordering::SeqCst);
        for update in updates {
            // `resolve` reads only immutable topology metadata, so every update validated
            // above resolves again here; re-resolving keeps publication allocation-free.
            let Ok((index, linear_gain)) = self.resolve(update) else {
                continue;
            };
            // Later updates to the same node in one batch build on the earlier stored value.
            let mut value = self.value(index);
            value.control = update.control;
            if let Some(linear_gain) = linear_gain {
                value.linear_gain = linear_gain;
            }
            self.cells[index]
                .value
                .store(value.encode(), Ordering::SeqCst);
        }
        self.published.store(revision, Ordering::SeqCst);
        if let Some(clock) = self.clock.get() {
            clock.published.fetch_max(revision, Ordering::Release);
        }
        self.epoch.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Resolves an update's cell index and replacement linear gain; `None` keeps the gain.
    fn resolve(
        &self,
        update: &RuntimeControlUpdate,
    ) -> Result<(usize, Option<f32>), RuntimeControlError> {
        let index = self
            .index(&update.node_id)
            .ok_or_else(|| RuntimeControlError::UnknownNode(update.node_id.clone()))?;
        let cell = &self.cells[index];
        if update.control.enabled && !cell.prepared {
            return Err(RuntimeControlError::NeedsPreparation(
                update.node_id.clone(),
            ));
        }
        let Some(gain_db) = update.gain_db else {
            return Ok((index, None));
        };
        if !cell.supports_gain {
            return Err(RuntimeControlError::UnsupportedGain(update.node_id.clone()));
        }
        let gain = crate::dsp::db_to_linear(gain_db);
        if !gain_db.is_finite() || !gain.is_finite() {
            return Err(RuntimeControlError::InvalidGain(update.node_id.clone()));
        }
        Ok((index, Some(gain)))
    }

    pub(crate) fn bind_clock(
        &self,
        clock: &Arc<ControlRevisionClock>,
    ) -> Result<(), RuntimeControlError> {
        let existing = self.clock.get_or_init(|| Arc::clone(clock));
        if Arc::ptr_eq(existing, clock) {
            Ok(())
        } else {
            Err(RuntimeControlError::DifferentExchange)
        }
    }

    pub(crate) fn index(&self, node_id: &NodeId) -> Option<usize> {
        self.identities
            .binary_search_by(|(id, _)| id.cmp(node_id))
            .ok()
            .map(|index| self.identities[index].1)
    }

    fn value(&self, index: usize) -> ControlValue {
        ControlValue::decode(self.cells[index].value.load(Ordering::SeqCst))
    }

    pub(crate) fn values(&self) -> Box<[ControlValue]> {
        self.initial.clone()
    }

    pub(crate) fn sample(
        &self,
        current: &mut Box<[ControlValue]>,
        scratch: &mut Box<[ControlValue]>,
        sampled_epoch: &mut Option<u64>,
    ) -> bool {
        let epoch = self.epoch.load(Ordering::SeqCst);
        if epoch & 1 != 0 || *sampled_epoch == Some(epoch) {
            return false;
        }
        for (index, value) in scratch.iter_mut().enumerate() {
            *value = self.value(index);
        }
        let revision = self.published.load(Ordering::SeqCst);
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return false;
        }
        std::mem::swap(current, scratch);
        *sampled_epoch = Some(epoch);
        self.applied.store(revision, Ordering::Release);
        if let Some(clock) = self.clock.get() {
            clock.applied.fetch_max(revision, Ordering::Release);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GainConfig, GraphCompiler, GraphDocument, NodeSpec,
        RenderFormat,
    };

    fn bank() -> RuntimeControlBank {
        let nodes = ["one", "two"].map(|id| NodeSpec {
            id: NodeId::from(id),
            label: None,
            control: NodeControl::default(),
            config: NodeConfig::Gain(GainConfig {
                channels: 2,
                gain_db: 0.0,
            }),
        });
        let execution = GraphCompiler::default()
            .compile(
                &GraphDocument::new(1, nodes.to_vec(), vec![]),
                CompileContext {
                    format: RenderFormat {
                        sample_rate: 48_000,
                        block_frames: 128,
                    },
                    limits: CompileLimits::default(),
                },
            )
            .unwrap();
        RuntimeControlBank::new(&execution, &[true, true])
    }

    fn updates(enabled: bool) -> [RuntimeControlUpdate; 2] {
        ["one", "two"].map(|id| RuntimeControlUpdate {
            node_id: NodeId::from(id),
            control: NodeControl {
                enabled,
                muted: !enabled,
            },
            gain_db: Some(if enabled { 0.0 } else { -6.0 }),
        })
    }

    #[test]
    fn odd_epoch_defers_complete_batch_without_waiting_for_the_writer() {
        let bank = bank();
        let mut current = bank.values();
        let mut scratch = current.clone();
        let mut sampled_epoch = None;
        bank.sample(&mut current, &mut scratch, &mut sampled_epoch);
        assert_eq!(bank.applied_revision(), 1);
        let _writer = bank.writer.lock();
        bank.epoch.store(1, Ordering::SeqCst);
        bank.cells[0].value.store(
            ControlValue {
                control: NodeControl {
                    enabled: false,
                    muted: true,
                },
                linear_gain: 0.5,
            }
            .encode(),
            Ordering::SeqCst,
        );
        bank.sample(&mut current, &mut scratch, &mut sampled_epoch);
        assert!(current.iter().all(|value| value.control.enabled));
        assert_eq!(bank.applied_revision(), 1);
        assert_eq!(sampled_epoch, Some(0));
    }

    #[test]
    fn invalid_targets_gain_and_revision_leave_complete_batch_unchanged() {
        let bank = bank();
        let mut batch = updates(false);
        batch[1].node_id = NodeId::from("removed");
        assert_eq!(
            bank.publish(2, &batch),
            Err(RuntimeControlError::UnknownNode(NodeId::from("removed")))
        );
        batch = updates(false);
        batch[1].gain_db = Some(f32::NAN);
        assert_eq!(
            bank.publish(2, &batch),
            Err(RuntimeControlError::InvalidGain(NodeId::from("two")))
        );
        assert_eq!(
            bank.publish(1, &updates(false)),
            Err(RuntimeControlError::StaleRevision {
                current: 1,
                proposed: 1
            })
        );
        assert_eq!(
            bank.snapshot(&NodeId::from("one")),
            Some((NodeControl::default(), 1.0))
        );
        assert_eq!(bank.published_revision(), 1);
        assert_eq!(bank.applied_revision(), 0);
    }

    #[test]
    fn concurrent_publications_never_mix_reference_values_or_acknowledge_half_a_batch() {
        let bank = Arc::new(bank());
        let mut current = bank.values();
        let mut scratch = current.clone();
        let mut sampled_epoch = None;
        let writer_bank = Arc::clone(&bank);
        let writer = std::thread::spawn(move || {
            for revision in 2..=1000 {
                writer_bank
                    .publish(revision, &updates(revision % 2 == 0))
                    .unwrap();
            }
        });
        for _ in 0..4000 {
            bank.sample(&mut current, &mut scratch, &mut sampled_epoch);
            assert_eq!(current[0].encode(), current[1].encode());
            let applied = bank.applied_revision();
            if applied > 1 {
                assert_eq!(current[0].control.enabled, applied.is_multiple_of(2));
            }
            assert!(applied <= bank.published_revision());
        }
        writer.join().unwrap();
        bank.sample(&mut current, &mut scratch, &mut sampled_epoch);
        assert_eq!(bank.applied_revision(), 1000);
        assert_eq!(current[0].encode(), current[1].encode());
    }

    #[test]
    fn unchanged_epoch_keeps_existing_block_snapshot_storage() {
        let bank = bank();
        let mut current = bank.values();
        let mut scratch = current.clone();
        let mut sampled_epoch = None;
        assert!(bank.sample(&mut current, &mut scratch, &mut sampled_epoch));
        let allocation = current.as_ptr();
        assert!(!bank.sample(&mut current, &mut scratch, &mut sampled_epoch));
        assert_eq!(current.as_ptr(), allocation);
        bank.publish(2, &updates(false)).unwrap();
        assert!(bank.sample(&mut current, &mut scratch, &mut sampled_epoch));
        assert_ne!(current.as_ptr(), allocation);
        assert_eq!(bank.applied_revision(), 2);
    }
}
