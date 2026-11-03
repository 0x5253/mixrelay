use super::*;
use std::{
    fs::File,
    os::fd::{AsRawFd, BorrowedFd},
};

unsafe extern "C" {
    fn xpc_dictionary_set_value(message: Xpc, key: *const c_char, value: Xpc);
    fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
}

fn descriptor() -> OwnedFd {
    let raw = crate::anonymous_memory::mr_shared_memory_create(4096, true);
    assert!(raw >= 0);
    unsafe { OwnedFd::from_raw_fd(raw) }
}
fn request() -> Message {
    Message::new(unsafe {
        mr_engine_control_create_clocked_source_request(c"com.example.broadcast".as_ptr(), 128, 2)
    })
    .unwrap()
}
fn attachment(source: &OwnedFd) -> Message {
    Message::new(populate_attachment(
        new_message(c"attach_source_transport").unwrap(),
        source.as_raw_fd(),
        4096,
        12,
        34,
    ))
    .unwrap()
}
struct Parsed(SourceAttachment);
impl Drop for Parsed {
    fn drop(&mut self) {
        unsafe { mr_engine_control_source_attachment_close(&raw mut self.0) };
    }
}
fn parse(message: &Message) -> Parsed {
    let mut result = Parsed(SourceAttachment::default());
    assert!(unsafe { mr_engine_control_parse_source_attachment(message.0, &raw mut result.0) });
    result
}

#[test]
fn clocked_request_preserves_identity_geometry_and_version() {
    let message = request();
    let mut parsed = SourceRequest::default();
    assert!(unsafe { mr_engine_control_parse_source_request(message.0, &raw mut parsed) });
    assert_eq!(parsed.transport_version, crate::application_clock::VERSION);
    assert_eq!(parsed.capacity_frames, 128);
    assert_eq!(parsed.channels, 2);
    assert_eq!(
        unsafe { CStr::from_ptr(parsed.source_id.as_ptr()) },
        c"com.example.broadcast"
    );
    assert_eq!(std::mem::size_of::<SourceRequest>(), 140);
    assert_eq!(std::mem::offset_of!(SourceRequest, transport_version), 136);
    assert_eq!(std::mem::size_of::<SourceAttachment>(), 32);
    assert_eq!(std::mem::offset_of!(SourceAttachment, producer_lease), 16);
}

#[test]
fn only_explicit_typed_current_version_requests_are_accepted() {
    let current = u64::from(crate::application_clock::VERSION);
    for invalid in (0..=current + 1)
        .chain([u64::MAX])
        .filter(|value| *value != current)
    {
        let message = request();
        unsafe {
            xpc_dictionary_set_uint64(message.0, c"source_transport_version".as_ptr(), invalid);
        };
        let mut parsed = SourceRequest::default();
        assert!(!unsafe { mr_engine_control_parse_source_request(message.0, &raw mut parsed) });
        assert_eq!(parsed, SourceRequest::default());
    }
    let message = request();
    let text_version = CString::new(current.to_string()).unwrap();
    unsafe {
        xpc_dictionary_set_string(
            message.0,
            c"source_transport_version".as_ptr(),
            text_version.as_ptr(),
        );
    };
    let mut parsed = SourceRequest::default();
    assert!(!unsafe { mr_engine_control_parse_source_request(message.0, &raw mut parsed) });
    unsafe {
        xpc_dictionary_set_value(
            message.0,
            c"source_transport_version".as_ptr(),
            ptr::null_mut(),
        );
    };
    assert!(!unsafe { mr_engine_control_parse_source_request(message.0, &raw mut parsed) });
}

#[test]
fn malformed_request_geometry_and_identity_never_publish_partial_output() {
    for field in [c"capacity_frames", c"channels"] {
        for invalid in [0, u64::from(u32::MAX) + 1] {
            let message = request();
            unsafe { xpc_dictionary_set_uint64(message.0, field.as_ptr(), invalid) };
            let mut parsed = SourceRequest::default();
            assert!(!unsafe { mr_engine_control_parse_source_request(message.0, &raw mut parsed) });
            assert_eq!(parsed, SourceRequest::default());
        }
    }
    assert!(
        unsafe { mr_engine_control_create_clocked_source_request(c"".as_ptr(), 128, 2) }.is_null()
    );
    assert!(
        unsafe { mr_engine_control_create_clocked_source_request(c"source".as_ptr(), 0, 2) }
            .is_null()
    );
    assert!(
        unsafe { mr_engine_control_create_clocked_source_request(ptr::null(), 128, 2) }.is_null()
    );
    let overlong = CString::new(vec![b'x'; NAME_BYTES]).unwrap();
    assert!(
        unsafe { mr_engine_control_create_clocked_source_request(overlong.as_ptr(), 128, 2) }
            .is_null()
    );
    let invalid_utf8 = CString::new([0xff]).unwrap();
    assert!(
        unsafe { mr_engine_control_create_clocked_source_request(invalid_utf8.as_ptr(), 128, 2) }
            .is_null()
    );
    let maximum = CString::new(vec![b'x'; NAME_BYTES - 1]).unwrap();
    let message = Message::new(unsafe {
        mr_engine_control_create_clocked_source_request(maximum.as_ptr(), 128, 2)
    })
    .unwrap();
    assert_eq!(
        parse_request(message.0).unwrap().source_id[NAME_BYTES - 1],
        0
    );
}

#[test]
fn source_attachment_owns_fd_and_lease_metadata_together() {
    let source = descriptor();
    let message = attachment(&source);
    let parsed = parse(&message);
    assert_ne!(parsed.0.descriptor, source.as_raw_fd());
    assert_eq!(parsed.0.mapped_size, 4096);
    assert_eq!(parsed.0.producer_lease, 12);
    assert_eq!(parsed.0.source_epoch, 34);
    drop(source);
    drop(message);
    let duplicate = unsafe { BorrowedFd::borrow_raw(parsed.0.descriptor) }
        .try_clone_to_owned()
        .unwrap();
    assert!(File::from(duplicate).metadata().unwrap().len() >= 4096);
}

#[test]
fn attachment_requires_complete_positive_typed_metadata_before_fd_transfer() {
    let source = descriptor();
    for key in [
        c"producer_lease",
        c"source_epoch",
        c"mapped_size",
        c"source_transport_version",
    ] {
        for wrong_type in [false, true] {
            let message = attachment(&source);
            unsafe {
                if wrong_type {
                    xpc_dictionary_set_string(message.0, key.as_ptr(), c"2".as_ptr());
                } else {
                    xpc_dictionary_set_uint64(message.0, key.as_ptr(), 0);
                }
            }
            let mut output = SourceAttachment {
                descriptor: -1,
                mapped_size: 99,
                producer_lease: 88,
                source_epoch: 77,
            };
            assert!(!unsafe {
                mr_engine_control_parse_source_attachment(message.0, &raw mut output)
            });
            assert_eq!(
                (
                    output.descriptor,
                    output.mapped_size,
                    output.producer_lease,
                    output.source_epoch
                ),
                (-1, 99, 88, 77)
            );
        }
    }
    let message = attachment(&source);
    unsafe {
        xpc_dictionary_set_string(
            message.0,
            c"operation".as_ptr(),
            c"attach_transport".as_ptr(),
        );
    };
    assert!(!unsafe {
        mr_engine_control_parse_source_attachment(message.0, &mut SourceAttachment::default())
    });
}

#[test]
fn malformed_source_fd_and_oversized_region_are_rejected() {
    let source = descriptor();
    let message = attachment(&source);
    unsafe {
        xpc_dictionary_set_string(message.0, c"transport_fd".as_ptr(), c"not an fd".as_ptr());
    };
    let mut output = SourceAttachment::default();
    assert!(!unsafe { mr_engine_control_parse_source_attachment(message.0, &raw mut output) });
    assert_eq!(output.descriptor, -1);
    let message = attachment(&source);
    unsafe {
        xpc_dictionary_set_uint64(message.0, c"mapped_size".as_ptr(), u64::from(u32::MAX) + 1);
    };
    assert!(!unsafe { mr_engine_control_parse_source_attachment(message.0, &raw mut output) });
}

#[test]
fn reply_validation_rejects_missing_lease_and_wrong_request_without_taking_fd() {
    let source = descriptor();
    let request = request();
    for (size, lease, epoch) in [(0, 1, 1), (4096, 0, 1), (4096, 1, 0)] {
        assert!(
            unsafe {
                mr_engine_control_create_source_attachment_reply(
                    request.0,
                    source.as_raw_fd(),
                    size,
                    lease,
                    epoch,
                )
            }
            .is_null()
        );
    }
    unsafe { xpc_dictionary_set_uint64(request.0, c"source_transport_version".as_ptr(), 0) };
    assert!(
        unsafe {
            mr_engine_control_create_source_attachment_reply(
                request.0,
                source.as_raw_fd(),
                4096,
                1,
                1,
            )
        }
        .is_null()
    );
    assert!(source.try_clone().is_ok());
}

#[test]
fn real_xpc_reply_failure_and_repeated_parse_close_release_all_descriptors() {
    if std::env::var_os("MR_SOURCE_XPC_FD_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_control::tests::real_xpc_reply_failure_and_repeated_parse_close_release_all_descriptors", "--test-threads=1"])
            .env("MR_SOURCE_XPC_FD_CHILD", "1").status().unwrap();
        assert!(status.success());
        return;
    }
    let source = descriptor();
    let request = request();
    let valid = attachment(&source);
    let malformed = attachment(&source);
    unsafe { xpc_dictionary_set_uint64(malformed.0, c"producer_lease".as_ptr(), 0) };
    let probe = source.try_clone().unwrap();
    let expected = probe.as_raw_fd();
    drop(probe);
    for _ in 0..100 {
        // A real local XPC dictionary has no remote reply right.
        assert!(
            unsafe {
                mr_engine_control_create_source_attachment_reply(
                    request.0,
                    source.as_raw_fd(),
                    4096,
                    1,
                    1,
                )
            }
            .is_null()
        );
        let mut invalid = SourceAttachment::default();
        assert!(!unsafe {
            mr_engine_control_parse_source_attachment(malformed.0, &raw mut invalid)
        });
        let mut parsed = parse(&valid);
        let owned = parsed.0.descriptor;
        assert!(unsafe { fcntl(owned, 1) } >= 0);
        unsafe { mr_engine_control_source_attachment_close(&raw mut parsed.0) };
        assert_eq!(unsafe { fcntl(owned, 1) }, -1);
        unsafe { mr_engine_control_source_attachment_close(&raw mut parsed.0) };
        let probe = source.try_clone().unwrap();
        assert_eq!(
            probe.as_raw_fd(),
            expected,
            "source XPC codec leaked a descriptor"
        );
    }
}
