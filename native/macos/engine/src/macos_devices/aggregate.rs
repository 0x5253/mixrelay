//! Private Aggregate ownership, admission, readiness budget and retirement.
//! Native calls perform one operation; Rust retains every partially created owner.
use super::{
    admission::{Admission, Permit},
    group_plan::GroupPlan,
    host::Host,
    retirement::{Retirable, Retirement},
};
use std::{ffi::c_void, ptr::NonNull, sync::Arc, thread, time::Duration};

static GROUPS: Admission = Admission::new(32);
static RETIRED: Retirement<GroupOwner> = Retirement::new();

#[cfg(test)]
#[cfg(feature = "native-lab")]
mod native_tests;

struct GroupOwner {
    pointer: Option<NonNull<c_void>>,
    host: Arc<dyn Host>,
    _permit: Permit<'static>,
}
// SAFETY: transferred only after the last IOProc owner retires; no callback or
// query can access a group in RETIRED. The control-side list serializes retries.
unsafe impl Send for GroupOwner {}
impl Retirable for GroupOwner {
    fn try_release(&mut self) -> bool {
        if let Some(pointer) = self.pointer {
            if !self.host.destroy_group(pointer) {
                return false;
            }
            self.pointer = None;
        }
        true
    }
}

/// Every native registration retains this Arc until callback teardown succeeds.
pub(super) struct Aggregate {
    pub(super) pointer: NonNull<c_void>,
    pub(super) members: Vec<String>,
    host: Arc<dyn Host>,
    owner: Option<GroupOwner>,
}
// SAFETY: immutable metadata and native property reads; Arc excludes retirement
// during a query. Destruction transfers exclusive ownership to the control list.
unsafe impl Send for Aggregate {}
unsafe impl Sync for Aggregate {}
impl Drop for Aggregate {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            RETIRED.retire(owner);
        }
    }
}
impl Aggregate {
    pub(super) fn active_owners() -> usize {
        GROUPS.in_use()
    }
    pub(super) fn ready(&self) -> bool {
        self.host.group_ready(self.pointer)
    }
    pub(super) fn reclaim_retired() {
        RETIRED.reclaim();
    }

    pub(super) fn create(
        members: Vec<String>,
        host: &Arc<dyn Host>,
    ) -> Result<Option<Arc<Self>>, String> {
        let Some(plan) = GroupPlan::prepare(&members, |uid| host.member_info(uid))? else {
            return Ok(None);
        };
        let permit = GROUPS
            .acquire()
            .ok_or("CoreAudio private group limit reached; native retirement pending")?;
        let (pointer, status) = host.create_group(&plan);
        let Some(pointer) = pointer else {
            return if status == 1 {
                Ok(None)
            } else {
                Err("CoreAudio private group allocation failed".into())
            };
        };
        // Install the Rust guard BEFORE checking creation status. The C boundary
        // transfers partial objects too, because cleanup may itself fail.
        let group = Self {
            pointer,
            members,
            host: host.clone(),
            owner: Some(GroupOwner {
                pointer: Some(pointer),
                host: host.clone(),
                _permit: permit,
            }),
        };
        match status {
            0 => {
                if !confirm_ready(
                    || group.ready(),
                    || thread::sleep(Duration::from_millis(20)),
                ) {
                    return Err(
                        "CoreAudio private aggregate clock-policy/geometry readback timed out"
                            .into(),
                    );
                }
                Ok(Some(Arc::new(group)))
            }
            1 => Ok(None), // Incompatible rates/user aggregate: retain independent bridges.
            _ => Err("CoreAudio private aggregate native creation failed".into()),
        }
    }
}

/// A bounded readiness policy, not a retry of failed native creation. No `IOProc`
/// is registered until the already-created object's complete geometry is visible.
fn confirm_ready(mut ready: impl FnMut() -> bool, mut wait: impl FnMut()) -> bool {
    for attempt in 0..100 {
        if ready() {
            return true;
        }
        if attempt < 99 {
            wait();
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::super::{
        DeviceSet, MacDevices,
        device::Device,
        simulated::{CAPTURED, SimulatedHost},
        wait_until,
        worker_retirement::WORKERS,
    };
    use super::*;
    use mixrelay_audio_graph::{
        AudioDeviceInputConfig, AudioDeviceOutputConfig, DeviceSelector, GraphDocument, NodeConfig,
        NodeControl, NodeSpec,
    };
    use mixrelay_audio_io::process::InputActivity;
    use mixrelay_engine_core::{GraphResourceBackend, PlaybackBindings::Preserve};
    use std::sync::atomic::Ordering;

    #[test]
    fn readiness_is_bounded_and_never_accepts_an_unconfirmed_object() {
        let mut checks = 0;
        let mut waits = 0;
        assert!(!confirm_ready(
            || {
                checks += 1;
                false
            },
            || waits += 1
        ));
        assert_eq!((checks, waits), (100, 99));
        let mut checks = 0;
        let mut waits = 0;
        assert!(confirm_ready(
            || {
                checks += 1;
                checks == 3
            },
            || waits += 1
        ));
        assert_eq!((checks, waits), (3, 2));
    }

    #[test]
    fn partial_native_creation_returns_admission_after_rejected_members() {
        let simulated = SimulatedHost::new();
        simulated.plug("valid", 2, 48_000, CAPTURED);
        let host: Arc<dyn Host> = simulated;
        // This test's own bound: concurrent tests cannot change its accounting.
        let admission: &'static Admission = Box::leak(Box::new(Admission::new(32)));
        // Production names are valid Rust strings, so use an explicit ABI fault: the second
        // UID is rejected after the first was adopted, before any member is queried.
        let plan = GroupPlan {
            names: vec![c"valid".into(), c"\xff".into()],
            facts: vec![super::super::group_plan::MemberInfo::default(); 2],
        };
        let retire_partial = |permit| {
            let (pointer, status) = host.create_group(&plan);
            assert_eq!(status, 2);
            RETIRED.retire(GroupOwner {
                pointer: Some(pointer.expect("partial allocation transferred")),
                host: host.clone(),
                _permit: permit,
            });
        };
        // Lazily initialized retirement storage allocates once, outside the measurement.
        retire_partial(admission.acquire().unwrap());
        // 128 partial owners exceed the 32-group bound, so each retirement returned its slot.
        // The same thread allocates and releases every Rust owner.
        let allocation = allocation_counter::measure(|| {
            for _ in 0..128 {
                retire_partial(admission.acquire().expect("partial owners must retire"));
                assert_eq!(admission.in_use(), 0);
            }
        });
        assert!(allocation.count_total > 0, "creation allocates its owner");
        assert_eq!(
            (allocation.count_current, allocation.bytes_current),
            (0, 0),
            "partial group owners leaked"
        );
    }

    fn duplex(microphone: &str, speaker: &str) -> GraphDocument {
        let selector = |uid: &str| DeviceSelector::PersistentUid { uid: uid.into() };
        let node = |id: &str, config| NodeSpec {
            control: NodeControl::default(),
            id: id.into(),
            label: None,
            config,
        };
        GraphDocument::new(
            1,
            vec![
                node(
                    "input",
                    NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                        selector: selector(microphone),
                        output_channels: 2,
                        gain_db: 0.0,
                    }),
                ),
                node(
                    "output",
                    NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                        selector: selector(speaker),
                        input_channels: 2,
                    }),
                ),
            ],
            vec![],
        )
    }

    /// Drops one owner and waits until its worker unregistered and released `Native`.
    fn retire(device: Arc<Device>) {
        let native = Arc::downgrade(&device.native);
        drop(device);
        wait_until(|| {
            WORKERS.reclaim();
            native.upgrade().is_none()
        });
    }

    #[test]
    fn private_group_members_share_one_clock_until_the_last_owner_retires() {
        let host = SimulatedHost::new();
        let microphone = host.plug("io.mixrelay.test.microphone", 2, 48_000, CAPTURED);
        let speaker = host.plug("io.mixrelay.test.speaker", 2, 48_000, 2.0 * CAPTURED);
        let status = Arc::new(mixrelay_engine_core::session::AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let resources = MacDevices::with_host(status, 48_000, 256, host.clone());
        resources
            .reconcile(&duplex(&microphone, &speaker), Preserve)
            .unwrap();
        let mut active = DeviceSet::default();
        resources.activation().unwrap().activate(&mut active);
        resources.reclaim();
        let [aggregate] = host.groups()[..] else {
            panic!("two physical members need one private group");
        };
        let (input, output) = (active.0["input"].clone(), active.0["output"].clone());
        for (device, offset) in [(&input, 2), (&output, 0)] {
            // The output clock leads the plan, and both registrations follow the aggregate.
            let group = device.native.group.as_ref().expect("group member");
            assert_eq!(group.members, [speaker.clone(), microphone.clone()]);
            assert_eq!(device.clock_identity.load(Ordering::Acquire), aggregate);
            // The production readback admits the aggregate and projects each member's slice.
            let binding = device.native.host.direct_binding(&device.native);
            assert_eq!(
                binding.map(|binding| (binding.device, binding.offset, binding.total)),
                Some((aggregate, offset, 4))
            );
        }
        // One clock identity lets the output's pacing clock align the input without SRC,
        // and the input captures only its own member's projection of the aggregate.
        let epoch = input.epoch.load(Ordering::Acquire);
        active.set_clock_master(Some(output.identity));
        assert!(input.clock_aligned.load(Ordering::Acquire));
        // The worker rebuilds its bridge for the aligned clock before capture resumes.
        wait_until(|| input.epoch.load(Ordering::Acquire) != epoch && input.ready());
        let held = host.hold_callbacks();
        let (mut pcm, mut captured) = ([0.0; 64], Vec::new());
        for _ in 0..4 {
            host.callback(&input.native)
                .expect("started aggregate registration");
            // One 512-frame callback fills at most 16 reads of 32 frames.
            for _ in 0..16 {
                let read = active.read("input", &mut pcm, 0).unwrap();
                assert_ne!(read.activity, InputActivity::Inactive, "input offline");
                captured.extend_from_slice(&pcm[..read.frames as usize * 2]);
            }
        }
        drop(held);
        // Past the bridge's priming silence, every sample is the microphone's, bit-exact.
        let settled = captured.len().saturating_sub(512);
        assert!(
            captured.len() >= 1024,
            "input captured {} samples",
            captured.len()
        );
        assert!(
            captured[settled..]
                .iter()
                .all(|sample| sample.to_bits() == CAPTURED.to_bits()),
            "input read beyond its member projection: {:?}",
            &captured[settled..]
        );
        resources
            .reconcile(&GraphDocument::new(2, vec![], vec![]), Preserve)
            .unwrap();
        resources.activation().unwrap().activate(&mut active);
        resources.reclaim();
        let group = Arc::downgrade(input.native.group.as_ref().expect("group member"));
        retire(input);
        assert!(
            group.upgrade().is_some(),
            "output registration still owns group"
        );
        assert_eq!(host.groups(), [aggregate]);
        retire(output);
        // `Native` stops upgrading before its fields drop on the worker thread.
        wait_until(|| group.upgrade().is_none());
        Aggregate::reclaim_retired();
        assert!(host.groups().is_empty(), "retired group was destroyed");
    }
}
