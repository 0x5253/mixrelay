use super::*;

#[test]
fn independent_names_ids_rates_and_descriptor_ownership_round_trip() {
    let source = backing();
    let mut second = entry(&source, 2, "방송 출력");
    second.sample_rate = 96_000;
    let message = message(&[entry(&source, 1, "Ableton"), second]);
    let parsed = decode(&message).unwrap();
    assert_eq!(parsed.0.count, 2);
    assert_eq!(parsed.0.generation, 7);
    assert_eq!(entry_name(&parsed.0.entries[0]), Some(c"Ableton"));
    assert_eq!(
        entry_name(&parsed.0.entries[1]).unwrap().to_str().unwrap(),
        "방송 출력"
    );
    assert_eq!(parsed.0.entries[1].sample_rate, 96_000);
    let owned = parsed.0.entries[0].descriptor;
    assert_ne!(owned, source.as_raw_fd());
    assert_ne!(owned, parsed.0.entries[1].descriptor);
    drop(message);
    drop(source);
    let borrowed = unsafe { BorrowedFd::borrow_raw(owned) };
    assert!(valid_backing(
        &File::from(borrowed.try_clone_to_owned().unwrap()),
        4096
    ));
}

#[test]
fn empty_authoritative_inventory_and_close_are_valid() {
    let mut parsed = decode(&message(&[])).unwrap();
    assert_eq!(parsed.0.count, 0);
    assert_eq!(parsed.0.generation, 7);
    unsafe {
        mr_hal_control_inventory_snapshot_close(&raw mut parsed.0);
        mr_hal_control_inventory_snapshot_close(&raw mut parsed.0);
    }
    assert_eq!(parsed.0.generation, 0);
    assert!(parsed.0.entries.iter().all(|entry| entry.descriptor == -1));
}

#[test]
fn invalid_creators_never_take_caller_descriptor() {
    let source = backing();
    let valid = entry(&source, 1, "Input");
    let mut invalid = valid;
    invalid.display_name = [c_char::from_ne_bytes([0xff]); NAME_BYTES];
    for entries in [&[valid, valid][..], &[invalid][..]] {
        assert!(
            unsafe { mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), 1) }
                .is_null()
        );
    }
    assert!(
        unsafe { mr_hal_control_create_inventory_snapshot(ptr::null(), MAX_ENTRIES + 1, 1) }
            .is_null()
    );
    assert!(unsafe { mr_hal_control_create_inventory_snapshot(&raw const valid, 1, 0) }.is_null());
    assert!(valid_backing(
        &File::from(source.try_clone().unwrap()),
        4096
    ));
}

#[test]
fn regular_file_backing_is_rejected_by_creator_and_parser() {
    let fixture = mixrelay_test_support::FixtureDir::new("inventory-regular-backing")
        .expect("owned fixture directory");
    let file = File::create(fixture.join("backing")).expect("regular backing fixture");
    file.set_len(4096).expect("size regular backing fixture");
    let regular: OwnedFd = file.into();
    let invalid = entry(&regular, 1, "Regular file");
    // SAFETY: the entry borrows the live fixture FD for this synchronous call.
    assert!(
        unsafe { mr_hal_control_create_inventory_snapshot(&raw const invalid, 1, 1) }.is_null()
    );

    let source = backing();
    let message = message(&[entry(&source, 1, "Anonymous memory")]);
    // SAFETY: the message owns the dictionary; XPC duplicates the live fixture FD.
    unsafe {
        xpc_dictionary_set_fd(
            item(&message, 0),
            c"transport_fd".as_ptr(),
            regular.as_raw_fd(),
        );
    }
    unchanged_on_error(&message);
    assert_eq!(
        File::from(regular.try_clone().expect("caller still owns regular FD"))
            .metadata()
            .expect("fixture metadata")
            .len(),
        4096
    );
}

#[test]
fn rejects_duplicate_ids_without_partial_publication() {
    let source = backing();
    let message = message(&[entry(&source, 1, "One"), entry(&source, 2, "Two")]);
    unsafe {
        xpc_dictionary_set_data(
            item(&message, 1),
            c"endpoint_id".as_ptr(),
            [1_u8; 16].as_ptr().cast(),
            16,
        );
    };
    unchanged_on_error(&message);
}

#[test]
fn malformed_final_entry_releases_previously_decoded_fds() {
    // Descriptor slots are process-global; isolate the leak check from
    // unrelated parallel tests which also open files.
    if std::env::var_os("MR_INVENTORY_FD_TEST_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "control_inventory::tests::snapshot::malformed_final_entry_releases_previously_decoded_fds",
                "--test-threads=1",
            ])
            .env("MR_INVENTORY_FD_TEST_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let source = backing();
    let message = message(&[entry(&source, 1, "One"), entry(&source, 2, "Two")]);
    // A backing length mismatch happens after both FD duplicates were made.
    unsafe { xpc_dictionary_set_uint64(item(&message, 1), c"mapped_size".as_ptr(), 1 << 20) };
    // dup returns the lowest available descriptor. Holding a sentinel means
    // a leaked parser FD consumes the next slot and changes the second dup.
    let probe = source.try_clone().unwrap();
    let next = source.try_clone().unwrap();
    let expected = next.as_raw_fd();
    drop(next);
    for _ in 0..100 {
        unchanged_on_error(&message);
        let next = source.try_clone().unwrap();
        assert_eq!(
            next.as_raw_fd(),
            expected,
            "partial parse leaked a descriptor"
        );
    }
    drop(probe);
}

#[test]
fn strict_types_lengths_generation_and_frame_bound() {
    let source = backing();
    for key in [
        c"sample_rate",
        c"mapped_size",
        c"generation",
        c"transport_fd",
        c"display_name",
        c"endpoint_id",
    ] {
        let message = message(&[entry(&source, 1, "One")]);
        unsafe {
            if key == c"display_name" {
                xpc_dictionary_set_uint64(item(&message, 0), key.as_ptr(), 3);
            } else {
                xpc_dictionary_set_string(item(&message, 0), key.as_ptr(), c"wrong type".as_ptr());
            }
        };
        unchanged_on_error(&message);
    }
    let oversized = message(&[entry(&source, 1, "One")]);
    let array = field(oversized.0, c"entries");
    for _ in 0..MAX_ENTRIES {
        unsafe { xpc_array_append_value(array, item(&oversized, 0)) };
    }
    unchanged_on_error(&oversized);
    let generation = message(&[]);
    unsafe { xpc_dictionary_set_uint64(generation.0, c"generation".as_ptr(), 0) };
    unchanged_on_error(&generation);
}
