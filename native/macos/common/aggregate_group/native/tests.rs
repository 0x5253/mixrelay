use std::ffi::c_void;

use super::{AggregateGeometry, AggregateGroup, MemberInfo, NativeBinding, audio, cf, ffi};

fn geometry() -> AggregateGeometry {
    AggregateGeometry::new(&[MemberInfo::default(); 2], 0).expect("bounded empty members")
}

#[test]
fn retirement_failure_preserves_identity_and_uid_owners() {
    let mut group = AggregateGroup::new(geometry());
    group.device = 91;
    group.uids[0] = cf::Object::string(c"owned-test-member");
    let uid = group.uids[0].as_ref().expect("CF string").as_ptr();
    assert!(!group.retire(|device| {
        assert_eq!(device, 91);
        -1
    }));
    assert_eq!(group.device, 91);
    assert_eq!(
        group.uids[0].as_ref().expect("retained CF string").as_ptr(),
        uid
    );
    assert!(group.retire(|device| {
        assert_eq!(device, 91);
        0
    }));
    assert_eq!(group.device, 0);
    assert!(group.retire(|_| panic!("already retired must not call CoreAudio")));
}

#[test]
fn already_removed_native_resources_acknowledge_retirement() {
    for status in [i32::from_be_bytes(*b"!obj"), i32::from_be_bytes(*b"!dev")] {
        let mut group = AggregateGroup::new(geometry());
        group.device = 91;
        assert!(group.retire(|_| status));
        assert_eq!(group.device, 0);
    }
}

#[test]
fn creation_failure_returns_the_partial_owner_and_release_consumes_it() {
    let members = [MemberInfo::default(); 2];
    for (rejected, expected) in [
        (c"".as_ptr(), 1),
        (std::ptr::null(), 1),
        (c"\xff".as_ptr(), 2),
    ] {
        let names = [c"already-adopted-member".as_ptr(), rejected];
        let mut status = -1;
        // SAFETY: immutable complete arrays and terminated non-null strings remain
        // live; each second UID rejects before any CoreAudio query/device creation.
        let group = unsafe {
            ffi::mr_aggregate_group_create(names.as_ptr(), members.as_ptr(), 2, 0, &raw mut status)
        };
        assert!(!group.is_null());
        assert_eq!(status, expected);
        // SAFETY: creation transferred one initialized owner, not published to callbacks.
        unsafe {
            assert!((*group).uids[0].is_some());
            assert!((*group).uids[1].is_none());
            assert_eq!((*group).device, 0);
            assert!(ffi::mr_aggregate_group_try_destroy(group));
        }
    }
}

#[test]
fn native_create_error_retains_returned_device_until_destruction_acknowledges() {
    let mut group = AggregateGroup::new(geometry());
    group.uids[0] = cf::Object::string(c"first");
    group.uids[1] = cf::Object::string(c"second");
    let mut created = false;
    assert!(!group.create(|description, device| {
        assert!(description.value(c"subdevices").is_some());
        *device = 73;
        created = true;
        -1
    }));
    assert!(created);
    assert_eq!(group.device, 73);
    assert!(!group.retire(|device| {
        assert_eq!(device, 73);
        -1
    }));
    assert_eq!(group.device, 73);
    assert!(group.uids[..2].iter().all(Option::is_some));
    assert!(group.retire(|device| {
        assert_eq!(device, 73);
        0
    }));
    assert_eq!(group.device, 0);
}

#[test]
fn invalid_ffi_inputs_do_not_publish_or_modify_binding() {
    let mut status = -1;
    let mut binding = NativeBinding {
        device: 1,
        offset: 2,
        total: 3,
    };
    // SAFETY: null owners/inputs are explicitly accepted as invalid; result slots
    // are initialized and exclusive, and no native operation is invoked.
    unsafe {
        assert!(
            ffi::mr_aggregate_group_create(
                std::ptr::null(),
                std::ptr::null(),
                2,
                0,
                &raw mut status
            )
            .is_null()
        );
        assert_eq!(status, 1);
        assert!(!ffi::mr_aggregate_group_ready(std::ptr::null()));
        assert!(!ffi::mr_aggregate_group_binding(
            std::ptr::null(),
            c"member".as_ptr(),
            false,
            2,
            &raw mut binding
        ));
        assert!(ffi::mr_aggregate_group_try_destroy(std::ptr::null_mut()));
    }
    assert_eq!(
        binding,
        NativeBinding {
            device: 1,
            offset: 2,
            total: 3
        }
    );
}

#[test]
fn channel_metadata_parser_checks_extent_and_total_budget() {
    assert_eq!(audio::parse_channels(&[2, 2, 0, 3, 0], 40), Some(5));
    assert_eq!(audio::parse_channels(&[0], 8), Some(0));
    assert_eq!(audio::parse_channels(&[2, 2, 0, 3, 0], 39), None);
    assert_eq!(audio::parse_channels(&[1, 2049, 0], 24), None);
    assert_eq!(audio::parse_channels(&[u64::MAX], 8), None);
    assert_eq!(audio::parse_channels(&[0], 9), None);
    assert_eq!(audio::parse_channels(&[], 0), None);
}

#[test]
fn cf_containers_retain_children_and_type_check_reads() {
    let number = cf::Object::number(96).expect("CF number");
    let dictionary = cf::Object::dictionary([(c"drift quality", &number)]).expect("CF dictionary");
    drop(number);
    assert_eq!(
        dictionary
            .value(c"drift quality")
            .expect("retained child")
            .integer(),
        Some(96)
    );
    assert!(dictionary.value(c"missing").is_none());
    assert!(dictionary.len().is_none());
    let array = cf::Object::array(&[Some(dictionary)]).expect("CF array");
    assert_eq!(array.len(), Some(1));
    assert!(array.element(1).is_none());
    let child = array.element(0).expect("retained array member");
    drop(array);
    assert_eq!(
        child
            .value(c"drift quality")
            .expect("child after parent drop")
            .integer(),
        Some(96)
    );
}

#[test]
fn description_encodes_private_nonstacked_group_and_primary_drift_policy() {
    let mut group = AggregateGroup::new(geometry());
    group.uids[0] = cf::Object::string(c"first");
    group.uids[1] = cf::Object::string(c"second");
    let description = group.description().expect("aggregate dictionary");
    assert_eq!(
        description.value(c"private").expect("private").integer(),
        Some(1)
    );
    assert_eq!(
        description.value(c"stacked").expect("stacked").integer(),
        Some(0)
    );
    assert!(
        description
            .value(c"master")
            .expect("primary")
            .equal(group.uids[0].as_ref().expect("first UID"))
    );
    let members = description.value(c"subdevices").expect("member array");
    assert_eq!(members.len(), Some(2));
    for index in 0..2 {
        let member = members.element(index).expect("member dictionary");
        assert_eq!(
            member.value(c"drift").expect("drift").integer(),
            Some(i32::from(index != 0))
        );
        assert_eq!(
            member.value(c"drift quality").expect("quality").integer(),
            Some(0x60)
        );
    }
}

#[test]
fn cf_property_error_and_malformed_size_release_returned_references() {
    unsafe extern "C" {
        fn CFRetain(value: *const c_void) -> *const c_void;
        fn CFGetRetainCount(value: *const c_void) -> isize;
    }
    let anchor = cf::Object::unique_uid().expect("unique heap string");
    // SAFETY: this private heap string stays owned by anchor throughout the test;
    // no other thread can retain/release it. Each simulated property owns +1.
    unsafe {
        let baseline = CFGetRetainCount(anchor.as_ptr());
        for (status, size) in [(-1, 8), (0, 4), (-1, 4)] {
            let returned = cf::Object::owned(CFRetain(anchor.as_ptr()));
            assert_eq!(CFGetRetainCount(anchor.as_ptr()), baseline + 1);
            assert!(audio::accept_object(returned, status, size).is_none());
            assert_eq!(CFGetRetainCount(anchor.as_ptr()), baseline);
        }
        let returned = cf::Object::owned(CFRetain(anchor.as_ptr()));
        let accepted = audio::accept_object(returned, 0, 8).expect("valid CF property");
        assert_eq!(CFGetRetainCount(anchor.as_ptr()), baseline + 1);
        drop(accepted);
        assert_eq!(CFGetRetainCount(anchor.as_ptr()), baseline);
    }
}
