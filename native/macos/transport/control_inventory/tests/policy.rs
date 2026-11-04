use super::*;

#[test]
fn transport_policy_codec_is_required_bounded_and_transactional() {
    let message = message(&[]);
    unsafe {
        xpc_dictionary_set_value(
            message.0,
            c"transport_compatibility".as_ptr(),
            ptr::null_mut(),
        );
    };
    let mut output = InventorySnapshot::default();
    assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    write_policy(message.0, &[]).unwrap();
    assert!(unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    assert!(output.transport_compatibility.identifiers().is_empty());
    let valid = [c"com.test.App".to_owned(), c"net.test.Other".to_owned()];
    write_policy(message.0, &valid).unwrap();
    assert!(unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    let expected = output.transport_compatibility.identifiers();
    assert_eq!(expected, ["com.test.App", "net.test.Other"]);
    for malformed in [
        vec![c"".to_owned()],
        vec![c"com..app".to_owned()],
        vec![c"com.app*".to_owned()],
        vec![c"com.test.App".to_owned(); 2],
        vec![CString::new("a".repeat(NAME_BYTES)).unwrap()],
        (0..=MAX_TRANSPORT_BUNDLES)
            .map(|index| CString::new(format!("com.app{index}")).unwrap())
            .collect(),
    ] {
        write_policy(message.0, &malformed).unwrap();
        assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        assert_eq!(output.transport_compatibility.identifiers(), expected);
        assert_eq!(output.generation, 7);
    }
    unsafe { xpc_dictionary_set_uint64(message.0, c"transport_compatibility".as_ptr(), 1) };
    assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    let array = Message::new(unsafe { xpc_array_create(ptr::null(), 0) }).unwrap();
    let item = new_message(c"wrong_type").unwrap();
    unsafe {
        xpc_array_append_value(array.0, item.0);
        xpc_dictionary_set_value(message.0, c"transport_compatibility".as_ptr(), array.0);
    }
    assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    assert_eq!(output.transport_compatibility.identifiers(), expected);
    let policy = ProducerPolicy::new(&["com.service.App".into()]).unwrap();
    assert!(ProducerPolicy::new(&["com..bad".into()]).is_none());
    assert!(ProducerPolicy::new(&vec!["com.app".into(); MAX_TRANSPORT_BUNDLES + 1]).is_none());
    assert!(ProducerPolicy::new(&["com.app".into(), "com.app".into()]).is_none());
    // SAFETY: an empty inventory borrows no descriptors.
    let owned = Message::new(unsafe { create_inventory_snapshot(&[], 7, &policy) }).unwrap();
    assert_eq!(
        decode(&owned)
            .unwrap()
            .0
            .transport_compatibility
            .identifiers(),
        ["com.service.App"]
    );
    assert!(
        decode(&self::message(&[]))
            .unwrap()
            .0
            .transport_compatibility
            .identifiers()
            .is_empty()
    );
}

#[test]
fn transport_policy_accepts_maximum_and_rejects_non_utf8() {
    let message = message(&[]);
    let valid: Vec<_> = (0..MAX_TRANSPORT_BUNDLES)
        .map(|index| CString::new(format!("com.app{index}")).unwrap())
        .collect();
    write_policy(message.0, &valid).unwrap();
    assert_eq!(
        decode(&message).unwrap().0.transport_compatibility.count,
        MAX_TRANSPORT_BUNDLES
    );
    write_policy(
        message.0,
        &[CString::new("a".repeat(NAME_BYTES - 1)).unwrap()],
    )
    .unwrap();
    assert!(decode(&message).is_some());
    write_policy(message.0, &[CString::new([0xff]).unwrap()]).unwrap();
    assert!(decode(&message).is_none());
}
