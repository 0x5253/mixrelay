use std::sync::atomic::Ordering;

use crate::{ProcessorControlRegistry, shared_transport::SharedHeader};

const UNKNOWN_LATENCY: u64 = 0;

/// Prevents mutations and failed processing from re-advertising an old cached value.
#[derive(Debug, Default)]
pub(crate) struct PublicationGate {
    needs_successful_snapshot: bool,
}

impl PublicationGate {
    pub(crate) fn cached(&self, frames: Option<u32>) -> Option<u32> {
        if self.needs_successful_snapshot {
            None
        } else {
            frames
        }
    }

    pub(crate) fn invalidate(&mut self) {
        self.needs_successful_snapshot = true;
    }

    pub(crate) fn process_failed(&mut self) {
        self.needs_successful_snapshot = true;
    }

    pub(crate) fn process_succeeded(&mut self, frames: Option<u32>) {
        if frames.is_some() {
            self.needs_successful_snapshot = false;
        }
    }
}

/// Cached latency information published by one active hosted processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessorLatencySnapshot {
    /// Prepared IPC block length in frames, not a wall-clock processing measurement.
    pub transport_frames: u32,
    /// Cached plug-in-reported intrinsic latency; `None` means unknown, not zero.
    pub intrinsic_frames: Option<u32>,
}

pub(crate) fn encode_intrinsic_frames(frames: Option<u32>) -> u64 {
    match frames {
        Some(frames) => u64::from(frames) + 1,
        None => UNKNOWN_LATENCY,
    }
}

pub(crate) fn decode_intrinsic_frames(encoded: u64) -> Option<u32> {
    if encoded == UNKNOWN_LATENCY {
        return None;
    }
    u32::try_from(encoded - 1).ok()
}

pub(crate) fn publish_intrinsic_frames(header: &SharedHeader, frames: Option<u32>) {
    header
        .intrinsic_latency_frames
        .store(encode_intrinsic_frames(frames), Ordering::Release);
}

impl ProcessorControlRegistry {
    /// Returns cached latency when `node_id` has one unambiguous active healthy owner.
    ///
    /// The transport component is the worker's prepared IPC block size. The intrinsic component
    /// is a plug-in-reported frame count and remains `None` when the format adapter cannot provide
    /// an authoritative cached value. Call from a control thread: owner lookup uses the registry
    /// lock, then reads shared atomics only; it does not query a plug-in.
    ///
    /// # Arguments
    ///
    /// * `node_id` - Active graph node whose hosted processor is being queried.
    ///
    /// # Returns
    ///
    /// `None` when there is no active healthy worker owner, otherwise its cached snapshot.
    #[must_use]
    pub fn latency_snapshot(&self, node_id: &crate::NodeId) -> Option<ProcessorLatencySnapshot> {
        let control = self.unique_active_control(node_id)?;
        let header = control.shared.header();
        if header.shutdown.load(Ordering::Acquire) != 0
            || header.worker_status.load(Ordering::Acquire) != crate::WORKER_READY
        {
            return None;
        }
        Some(ProcessorLatencySnapshot {
            transport_frames: header.prepared_block_frames(),
            intrinsic_frames: decode_intrinsic_frames(
                header.intrinsic_latency_frames.load(Ordering::Acquire),
            ),
        })
    }

    fn unique_active_control(
        &self,
        node_id: &crate::NodeId,
    ) -> Option<std::sync::Arc<crate::HostedProcessorControl>> {
        let mut controls = self
            .controls
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entries = controls.get_mut(node_id)?;
        entries.retain(|entry| entry.strong_count() != 0);
        let mut owner = None;
        for entry in entries.iter() {
            let Some(control) = entry.upgrade() else {
                continue;
            };
            if !control.active_owner.load(Ordering::Acquire) {
                continue;
            }
            if owner.is_some() {
                return None;
            }
            owner = Some(control);
        }
        owner
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::Ordering};

    use mixrelay_audio_engine::HostedAudioProcessor;
    use mixrelay_audio_graph::NodeId;
    use mixrelay_test_support::FixtureDir;

    use super::*;
    use crate::{
        AudioLayout, HostedProcessorControl, WORKER_FAILED, WORKER_READY,
        control::ProcessorControlRegistration,
        hosted::HostedExternalProcessor,
        shared_transport::{self, ParentAudio, WorkerAudio, WorkerControl},
    };

    struct RegisteredControl {
        _registration: ProcessorControlRegistration,
        control: Arc<HostedProcessorControl>,
        worker_control: WorkerControl,
        _worker_audio: WorkerAudio,
        _parent: ParentAudio,
    }

    fn register_control(
        registry: &ProcessorControlRegistry,
        node_id: &NodeId,
        directory: &FixtureDir,
        name: &str,
    ) -> RegisteredControl {
        let layout = AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 64,
        };
        let path = directory.join(name);
        let (parent, mailbox) = shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .expect("latency test transport should be created");
        let (worker_audio, worker_control) =
            shared_transport::worker(&path).expect("worker mapping should open the fixture");
        let control = Arc::new(HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let registration = registry.register(node_id, &control);
        RegisteredControl {
            _registration: registration,
            control,
            worker_control,
            _worker_audio: worker_audio,
            _parent: parent,
        }
    }

    fn ready(fixture: &RegisteredControl) {
        fixture.control.active_owner.store(true, Ordering::Release);
        fixture
            .worker_control
            .header()
            .worker_status
            .store(WORKER_READY, Ordering::Release);
    }

    fn processor_with_control(
        control: Arc<HostedProcessorControl>,
        directory: &FixtureDir,
    ) -> HostedExternalProcessor {
        HostedExternalProcessor {
            worker: None,
            shared: None,
            config_path: directory.join("unused-processor-config"),
            layout: AudioLayout {
                inputs: 2,
                outputs: 2,
                block_frames: 64,
            },
            dry_delay: crate::dry_delay::DryDelay::new(2, 2, 64)
                .expect("processor latency fixture delay should be created"),
            next_sequence: 1,
            timing: crate::timing::TimingProbe::default(),
            control: Some(control),
            registration: None,
        }
    }

    #[test]
    fn registry_requires_active_ready_and_non_shutdown_worker() {
        let registry = ProcessorControlRegistry::default();
        let node_id = NodeId::from("latency-owner");
        let directory = FixtureDir::new("processor-latency-owner")
            .expect("latency fixture directory should be created");
        let fixture = register_control(&registry, &node_id, &directory, "audio");
        let header = fixture.worker_control.header();

        assert_eq!(registry.latency_snapshot(&node_id), None);
        fixture.control.active_owner.store(true, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
        header.worker_status.store(WORKER_READY, Ordering::Release);
        assert_eq!(
            registry.latency_snapshot(&node_id),
            Some(ProcessorLatencySnapshot {
                transport_frames: 64,
                intrinsic_frames: None,
            })
        );
        header.shutdown.store(1, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
        header.shutdown.store(0, Ordering::Release);
        header.worker_status.store(WORKER_FAILED, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
        header.worker_status.store(WORKER_READY, Ordering::Release);
        fixture.control.active_owner.store(false, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
    }

    #[test]
    fn shared_publication_distinguishes_unknown_zero_max_and_updates_without_pcm() {
        let registry = ProcessorControlRegistry::default();
        let node_id = NodeId::from("latency-values");
        let directory = FixtureDir::new("processor-latency-values")
            .expect("latency fixture directory should be created");
        let fixture = register_control(&registry, &node_id, &directory, "audio");
        ready(&fixture);
        let header = fixture.worker_control.header();

        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            None
        );
        publish_intrinsic_frames(header, Some(0));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(0)
        );
        publish_intrinsic_frames(header, Some(u32::MAX));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(u32::MAX)
        );
        publish_intrinsic_frames(header, Some(127));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(127)
        );
        let mut gate = PublicationGate::default();
        gate.process_failed();
        publish_intrinsic_frames(header, gate.cached(Some(127)));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            None
        );
        gate.process_succeeded(Some(128));
        publish_intrinsic_frames(header, gate.cached(Some(128)));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(128)
        );
        publish_intrinsic_frames(header, None);
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            None
        );
        assert_eq!(
            decode_intrinsic_frames(encode_intrinsic_frames(Some(u32::MAX))),
            Some(u32::MAX)
        );
        assert_eq!(decode_intrinsic_frames(u64::MAX), None);
    }

    #[test]
    fn prepared_inactive_reader_tracks_only_its_healthy_cached_worker() {
        let registry = ProcessorControlRegistry::default();
        let node_id = NodeId::from("latency-prepared-candidate");
        let directory = FixtureDir::new("processor-latency-prepared-candidate")
            .expect("latency fixture directory should be created");
        let active = register_control(&registry, &node_id, &directory, "active-audio");
        ready(&active);
        publish_intrinsic_frames(active.worker_control.header(), Some(3));

        let candidate = register_control(&registry, &node_id, &directory, "candidate-audio");
        let candidate_header = candidate.worker_control.header();
        candidate_header
            .worker_status
            .store(WORKER_READY, Ordering::Release);
        publish_intrinsic_frames(candidate_header, Some(7));
        assert!(!candidate.control.active_owner.load(Ordering::Acquire));

        let processor = processor_with_control(Arc::clone(&candidate.control), &directory);
        let reader = processor
            .latency_source()
            .expect("a prepared worker should retain its latency source");
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .expect("the old worker remains the only active owner")
                .intrinsic_frames,
            Some(3)
        );
        assert_eq!(reader.frames(), Some(71));

        publish_intrinsic_frames(candidate_header, Some(0));
        assert_eq!(reader.frames(), Some(64));
        publish_intrinsic_frames(candidate_header, None);
        assert_eq!(reader.frames(), None);
        publish_intrinsic_frames(candidate_header, Some(11));
        assert_eq!(reader.frames(), Some(75));

        candidate_header
            .worker_status
            .store(WORKER_FAILED, Ordering::Release);
        assert_eq!(reader.frames(), None);
        candidate_header
            .worker_status
            .store(WORKER_READY, Ordering::Release);
        candidate_header.shutdown.store(1, Ordering::Release);
        assert_eq!(reader.frames(), None);
        candidate_header.shutdown.store(0, Ordering::Release);
        publish_intrinsic_frames(candidate_header, Some(u32::MAX));
        assert_eq!(reader.frames(), None);
    }

    #[test]
    fn retained_latency_reader_delays_transport_unlink_until_final_control_drop() {
        let directory = FixtureDir::new("processor-latency-reader-lifetime")
            .expect("latency fixture directory should be created");
        let shared_path = directory.join("owned-audio");
        let config_path = directory.join("worker-config");
        std::fs::write(&config_path, b"fixture").expect("worker config should be created");
        let layout = AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 64,
        };
        let (parent, mailbox) = shared_transport::create(
            &shared_path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .expect("owned latency transport should be created");
        let (worker_audio, worker_control) =
            shared_transport::worker(&shared_path).expect("simulated worker transport should open");
        let control = Arc::new(HostedProcessorControl::native(
            mailbox,
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        control.own_transport_file(shared_path.clone());
        let header = worker_control.header();
        header.worker_status.store(WORKER_READY, Ordering::Release);
        publish_intrinsic_frames(header, Some(5));

        let processor = HostedExternalProcessor {
            worker: None,
            shared: Some(parent),
            config_path: config_path.clone(),
            layout,
            dry_delay: crate::dry_delay::DryDelay::new(2, 2, 64)
                .expect("processor latency fixture delay should be created"),
            next_sequence: 1,
            timing: crate::timing::TimingProbe::default(),
            control: Some(Arc::clone(&control)),
            registration: None,
        };
        let reader = processor
            .latency_source()
            .expect("prepared worker should expose its exact latency source");
        assert_eq!(reader.frames(), Some(69));

        drop(processor);
        assert_eq!(
            reader.frames(),
            None,
            "processor drop marks the worker stopped"
        );
        assert!(
            shared_path.exists(),
            "the reader still owns the mapped control"
        );
        assert!(
            !config_path.exists(),
            "worker configuration is released promptly"
        );

        drop(worker_control);
        drop(worker_audio);
        drop(reader);
        assert!(
            shared_path.exists(),
            "the local control Arc still owns the mapping"
        );
        drop(control);
        assert!(
            !shared_path.exists(),
            "the final control drop unlinks the transport"
        );
    }

    #[test]
    fn handoff_requires_one_active_owner_and_never_falls_back_to_stale_data() {
        let registry = ProcessorControlRegistry::default();
        let node_id = NodeId::from("latency-replacement");
        let directory = FixtureDir::new("processor-latency-replacement")
            .expect("latency fixture directory should be created");
        let old = register_control(&registry, &node_id, &directory, "old-audio");
        ready(&old);
        publish_intrinsic_frames(old.worker_control.header(), Some(12));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(12)
        );

        let candidate = register_control(&registry, &node_id, &directory, "candidate-audio");
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(12)
        );
        ready(&candidate);
        publish_intrinsic_frames(candidate.worker_control.header(), Some(48));
        assert_eq!(registry.latency_snapshot(&node_id), None);
        old.worker_control
            .header()
            .worker_status
            .store(WORKER_FAILED, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
        old.control.active_owner.store(false, Ordering::Release);
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(48)
        );

        publish_intrinsic_frames(old.worker_control.header(), Some(99));
        assert_eq!(
            registry
                .latency_snapshot(&node_id)
                .unwrap()
                .intrinsic_frames,
            Some(48)
        );
        candidate
            .worker_control
            .header()
            .worker_status
            .store(WORKER_FAILED, Ordering::Release);
        assert_eq!(registry.latency_snapshot(&node_id), None);
    }

    #[test]
    fn publication_gate_keeps_failed_and_mutated_cache_unknown_until_authoritative_success() {
        let mut gate = PublicationGate::default();
        assert_eq!(gate.cached(Some(44)), Some(44));

        gate.process_failed();
        assert_eq!(gate.cached(Some(44)), None);
        assert_eq!(gate.cached(Some(45)), None);
        gate.process_succeeded(Some(46));
        assert_eq!(gate.cached(Some(46)), Some(46));

        gate.invalidate();
        assert_eq!(gate.cached(Some(46)), None);
        gate.process_succeeded(None);
        assert_eq!(gate.cached(Some(46)), None);
        gate.process_succeeded(Some(47));
        assert_eq!(gate.cached(Some(47)), Some(47));
    }

    #[test]
    fn expired_weak_owner_does_not_leave_a_latency_snapshot() {
        let registry = ProcessorControlRegistry::default();
        let node_id = NodeId::from("latency-expired");
        let directory = FixtureDir::new("processor-latency-expired")
            .expect("latency fixture directory should be created");
        {
            let fixture = register_control(&registry, &node_id, &directory, "audio");
            ready(&fixture);
            publish_intrinsic_frames(fixture.worker_control.header(), Some(23));
            assert_eq!(
                registry
                    .latency_snapshot(&node_id)
                    .unwrap()
                    .intrinsic_frames,
                Some(23)
            );
        }
        assert_eq!(registry.latency_snapshot(&node_id), None);
    }
}
