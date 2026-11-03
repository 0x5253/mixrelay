use std::ffi::{CStr, c_void};

use super::super::{AggregateGeometry, AggregateGroup, MemberInfo, NativeBinding, cf};
use super::{
    CreationStatus, initialize, mr_aggregate_group_create, mr_aggregate_group_try_destroy,
};

unsafe extern "C" {
    fn CFRetain(value: *const c_void) -> *const c_void;
    fn CFGetRetainCount(value: *const c_void) -> isize;
}

// Longer than any tagged-pointer string, so each UID is a heap object with a real count.
const PRIMARY: &CStr = c"io.mixrelay.test.aggregate-member.primary";
const SECONDARY: &CStr = c"io.mixrelay.test.aggregate-member.secondary";

/// An extra owned reference to `object`, so its count stays observable after the owner drops.
fn witness(object: &cf::Object) -> cf::Object {
    // SAFETY: `object` is a live CF reference; the retain is adopted exactly once.
    unsafe { cf::Object::owned(CFRetain(object.as_ptr())) }.expect("retained CF object")
}

fn references(object: &cf::Object) -> isize {
    // SAFETY: the witness keeps this private heap object alive for the query.
    unsafe { CFGetRetainCount(object.as_ptr()) }
}

#[test]
fn stale_member_facts_reject_creation_before_any_native_device() {
    let planned = |device| MemberInfo {
        device,
        transport: 1,
        rate: 48_000.0,
        input: 0,
        output: 2,
        alive: 1,
    };
    let primary = cf::Object::string(PRIMARY).expect("CF string");
    // Facts read for the plan, then a changed identity, geometry or rate when revalidated.
    for stale in [Some(0), Some(1), Some(2), None] {
        let mut group = AggregateGroup::new(
            AggregateGeometry::new(&[planned(10), planned(20)], 0).expect("planned geometry"),
        );
        let observe = |uid: &cf::Object| {
            let first = uid.equal(&primary);
            let mut fact = planned(if first { 10 } else { 20 });
            match stale.filter(|_| first) {
                Some(0) => fact.device = 0,
                Some(1) => fact.output += 1,
                Some(_) => fact.rate += 1.0,
                None => {}
            }
            Some(fact)
        };
        let mut description = None;
        let status = initialize(
            &mut group,
            [Some(PRIMARY), Some(SECONDARY)],
            observe,
            |created, device| {
                description = Some(witness(created));
                *device = 73;
                0
            },
        );
        if stale.is_some() {
            assert_eq!(status, CreationStatus::NativeFailure, "stale plan accepted");
            assert!(description.is_none(), "stale plan reached native creation");
            assert_eq!(group.device, 0);
        } else {
            assert_eq!(status, CreationStatus::Created);
            let created = description.as_ref().expect("native creation");
            assert_eq!(references(created), 1, "description leaked");
        }
        // The description, holding the members too, is gone: only the group and this test do.
        drop(description);
        let members = group.uids[..2]
            .iter()
            .map(|uid| witness(uid.as_ref().expect("adopted member UID")))
            .collect::<Vec<_>>();
        assert!(members.iter().all(|uid| references(uid) == 2));
        if stale.is_none() {
            assert!(group.retire(|device| {
                assert_eq!(device, 73);
                0
            }));
        }
        drop(group);
        assert!(
            members.iter().all(|uid| references(uid) == 1),
            "group leaked a member UID"
        );
    }
}

#[test]
fn partial_creation_releases_every_adopted_cf_reference_on_destruction() {
    let members = [MemberInfo::default(); 2];
    let names = [PRIMARY.as_ptr(), c"\xff".as_ptr()];
    let mut status = 0;
    // SAFETY: both arrays hold two initialized entries with terminated UIDs, live and
    // immutable for the call; the second UID is rejected before any CoreAudio query.
    let group = unsafe {
        mr_aggregate_group_create(names.as_ptr(), members.as_ptr(), 2, 0, &raw mut status)
    };
    assert_eq!(status, 2);
    // SAFETY: creation transferred this unpublished owner; it has no native device.
    let adopted = unsafe { &*group }.uids[0]
        .as_ref()
        .map(witness)
        .expect("first UID adopted");
    assert_eq!(references(&adopted), 2);
    // SAFETY: the same unpublished owner, consumed exactly once.
    assert!(unsafe { mr_aggregate_group_try_destroy(group) });
    assert_eq!(
        references(&adopted),
        1,
        "partial owner leaked a CF reference"
    );
}

#[test]
fn simulated_group_reads_back_its_plan_and_projects_each_member() {
    use super::simulated;
    use crate::device_io::test_observation::SimulatedHal;
    let hal = SimulatedHal::new();
    hal.plug("io.mixrelay.test.speaker", 2, 48_000, 0.5);
    hal.plug("io.mixrelay.test.microphone", 1, 48_000, 0.25);
    let names = [
        c"io.mixrelay.test.speaker".as_ptr(),
        c"io.mixrelay.test.microphone".as_ptr(),
    ];
    let mut facts = [MemberInfo::default(); 2];
    for (name, fact) in names.iter().zip(&mut facts) {
        // SAFETY: a terminated UID and one exclusive snapshot, both live for the call.
        assert!(unsafe { simulated::member_info(&hal, *name, fact) });
        assert_eq!(fact.transport, u32::from_be_bytes(*b"bltn"));
    }
    // SAFETY: one fact per terminated UID, immutable for the call.
    let (group, status) = unsafe { simulated::create(&hal, &names, facts.as_ptr(), 0) };
    assert_eq!(status, 0);
    let [aggregate] = hal.aggregates()[..] else {
        panic!("creation published no aggregate");
    };
    // SAFETY: the created owner stays retained and unmutated until its destruction below.
    unsafe {
        assert!(simulated::ready(&hal, group));
        let project = |uid: &CStr, input| simulated::binding(&hal, group, uid, input, 1);
        assert_eq!(
            project(c"io.mixrelay.test.microphone", true),
            Some(NativeBinding {
                device: aggregate,
                offset: 2,
                total: 3,
            })
        );
        assert_eq!(
            project(c"io.mixrelay.test.speaker", false).map(|binding| binding.offset),
            Some(0)
        );
        assert_eq!(project(c"io.mixrelay.test.absent", true), None);
        // A missing member, and its return as a new object, both break the planned readback.
        hal.unplug("io.mixrelay.test.microphone");
        assert!(!simulated::ready(&hal, group));
        assert_eq!(project(c"io.mixrelay.test.speaker", false), None);
        hal.replug("io.mixrelay.test.microphone");
        assert!(
            !simulated::ready(&hal, group),
            "stale member identity accepted"
        );
        assert!(simulated::try_destroy(&hal, group));
    }
    assert!(
        hal.aggregates().is_empty(),
        "retired aggregate was not destroyed"
    );
}

#[test]
fn simulated_creation_revalidates_members_before_publishing() {
    use super::simulated;
    use crate::device_io::test_observation::SimulatedHal;
    let hal = SimulatedHal::new();
    hal.plug("io.mixrelay.test.first", 2, 48_000, 0.5);
    hal.plug("io.mixrelay.test.second", 2, 48_000, 0.25);
    let names = [
        c"io.mixrelay.test.first".as_ptr(),
        c"io.mixrelay.test.second".as_ptr(),
    ];
    let mut facts = [MemberInfo::default(); 2];
    for (name, fact) in names.iter().zip(&mut facts) {
        // SAFETY: a terminated UID and one exclusive snapshot, both live for the call.
        assert!(unsafe { simulated::member_info(&hal, *name, fact) });
    }
    // The second member returns as a new object between planning and creation.
    hal.unplug("io.mixrelay.test.second");
    hal.replug("io.mixrelay.test.second");
    // SAFETY: one fact per terminated UID, immutable for the call.
    let (group, status) = unsafe { simulated::create(&hal, &names, facts.as_ptr(), 0) };
    assert_eq!(status, 2);
    assert!(
        hal.aggregates().is_empty(),
        "a stale plan published an aggregate"
    );
    // SAFETY: the partial owner transferred and has no native device.
    assert!(unsafe { simulated::try_destroy(&hal, group) });
}
