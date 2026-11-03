use std::ffi::{c_char, c_void};
use std::ptr::NonNull;

use super::{
    MAX_DEVICE_BYTES, MAX_PROPERTY_BYTES, OwnedString, UTF8, channel_count, decode_device_ids,
    read_property_bytes, read_string,
};

unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        bytes: *const c_char,
        encoding: u32,
    ) -> *mut c_void;
    fn CFRetain(value: *const c_void) -> *const c_void;
    fn CFGetRetainCount(value: *const c_void) -> isize;
    static kCFBooleanTrue: *const c_void;
}

fn owned_string(text: &str) -> OwnedString {
    let bytes = std::ffi::CString::new(text).unwrap();
    // SAFETY: the constructor copies the owned CString and returns a +1 reference.
    let pointer = unsafe { CFStringCreateWithCString(std::ptr::null(), bytes.as_ptr(), UTF8) };
    OwnedString(NonNull::new(pointer).unwrap())
}

#[test]
fn device_id_decoder_rejects_partial_u32_and_preserves_native_ids() {
    assert!(decode_device_ids(&[]).unwrap().is_empty());
    assert!(decode_device_ids(&[1, 2, 3]).is_err());
    let bytes: Vec<_> = [7_u32, 13, 1024]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    assert_eq!(decode_device_ids(&bytes).unwrap(), [7, 13, 1024]);
}

#[test]
fn channel_count_checks_variable_list_bounds_and_saturates_overflow() {
    assert_eq!(channel_count(&[]), None);
    assert_eq!(channel_count(&[0; 8]), Some(0));
    let mut bytes = [0_u8; 40];
    bytes[..4].copy_from_slice(&2_u32.to_ne_bytes());
    bytes[8..12].copy_from_slice(&7_u32.to_ne_bytes());
    bytes[24..28].copy_from_slice(&3_u32.to_ne_bytes());
    assert_eq!(channel_count(&bytes), Some(10));
    assert_eq!(channel_count(&bytes[..39]), None);
    bytes[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert_eq!(channel_count(&bytes), Some(u32::MAX));
    bytes[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert_eq!(channel_count(&bytes), None);
}

#[test]
fn retained_cf_string_conversion_preserves_utf8_and_releases_owned_reference() {
    for text in ["Interface", "오디오 장치", ""] {
        let owned = owned_string(text);
        assert_eq!(owned.text().as_deref(), Some(text));
        drop(owned);
    }
}

#[test]
fn property_reads_use_only_the_returned_extent_after_inventory_shrinks() {
    let value = read_property_bytes(12, MAX_DEVICE_BYTES, |words, size| {
        assert_eq!(words.len(), 2);
        assert_eq!(*size, 12);
        let mut bytes = [0_u8; 8];
        bytes[..4].copy_from_slice(&7_u32.to_ne_bytes());
        bytes[4..].copy_from_slice(&13_u32.to_ne_bytes());
        words[0] = u64::from_ne_bytes(bytes);
        words[1] = u64::MAX;
        *size = 8;
        0
    })
    .unwrap();
    assert_eq!(decode_device_ids(value.as_bytes()).unwrap(), [7, 13]);
    let truncated = read_property_bytes(8, MAX_DEVICE_BYTES, |_, size| {
        *size = 7;
        0
    })
    .unwrap();
    assert!(decode_device_ids(truncated.as_bytes()).is_err());
}

#[test]
fn oversized_failed_and_growing_property_reads_do_not_publish_storage() {
    assert!(
        read_property_bytes(MAX_DEVICE_BYTES + 1, MAX_DEVICE_BYTES, |_, _| {
            panic!("oversized property must be rejected before read");
        })
        .is_err()
    );
    assert!(
        read_property_bytes(8, MAX_DEVICE_BYTES, |_, size| {
            *size = 9;
            0
        })
        .is_err()
    );
    assert!(read_property_bytes(8, MAX_DEVICE_BYTES, |_, _| -1).is_err());
    let empty = read_property_bytes(0, MAX_DEVICE_BYTES, |_, _| {
        panic!("zero-size inventory must not read into absent storage");
    })
    .unwrap();
    assert!(empty.as_bytes().is_empty());
    let maximum = read_property_bytes(MAX_DEVICE_BYTES, MAX_DEVICE_BYTES, |_, _| 0).unwrap();
    assert_eq!(decode_device_ids(maximum.as_bytes()).unwrap().len(), 1024);
}

#[test]
fn channel_count_never_reads_a_buffer_beyond_the_returned_extent() {
    let value = read_property_bytes(40, MAX_PROPERTY_BYTES, |words, size| {
        words[0] = 2;
        words[1] = 7;
        words[3] = 3;
        *size = 24;
        0
    })
    .unwrap();
    assert_eq!(channel_count(value.as_bytes()), None);
}

#[test]
fn string_output_releases_its_transferred_reference_on_success_and_every_failure() {
    // A heap-backed value avoids immortal/tagged CF strings. The test owns one
    // independent reference and observes the extra transferred retain returning.
    let text = "Retained device identity ".repeat(40);
    let owner = owned_string(&text);
    let pointer = owner.0.as_ptr();
    // SAFETY: owner keeps this heap-backed CF object alive throughout observation.
    let retained = unsafe { CFGetRetainCount(pointer) };
    let pointer_size = u32::try_from(size_of::<*mut c_void>()).unwrap();
    for (status, reported) in [(0, pointer_size), (-1, pointer_size), (0, 0)] {
        // SAFETY: this explicit +1 models a property transferring one owned reference.
        unsafe { CFRetain(pointer) };
        // SAFETY: the closure transfers precisely the extra retained reference above.
        let result = unsafe {
            read_string(|output, size| {
                *output = pointer;
                *size = reported;
                status
            })
        };
        if status == 0 && reported != 0 {
            assert_eq!(result.as_deref(), Some(text.as_str()));
        } else {
            assert!(result.is_none());
        }
        // SAFETY: the independent owner remains alive after the transferred retain ends.
        assert_eq!(unsafe { CFGetRetainCount(pointer) }, retained);
    }
    // SAFETY: neither closure writes a pointer, so no reference is transferred.
    assert!(unsafe { read_string(|_, _| 0) }.is_none());
    // SAFETY: error with unchanged null output transfers no reference.
    assert!(unsafe { read_string(|_, _| -1) }.is_none());
}

#[test]
fn oversized_string_conversion_releases_the_transferred_reference() {
    let owner = owned_string(&"x".repeat(MAX_PROPERTY_BYTES as usize + 1));
    let pointer = owner.0.as_ptr();
    // SAFETY: owner holds the independent retain until all observations finish.
    let retained = unsafe { CFGetRetainCount(pointer) };
    // SAFETY: create an extra retained output for the synthetic property transfer.
    unsafe { CFRetain(pointer) };
    assert!(
        // SAFETY: this closure transfers the extra retain into the bounded reader.
        unsafe {
            read_string(|output, _| {
                *output = pointer;
                0
            })
        }
        .is_none()
    );
    // SAFETY: the original owner is still alive after the reader releases its retain.
    assert_eq!(unsafe { CFGetRetainCount(pointer) }, retained);
}

#[test]
fn mistyped_retained_property_is_released_without_string_operations() {
    // SAFETY: CoreFoundation's boolean singleton is live; the explicit retain is
    // transferred to read_string and released there, even though its type is wrong.
    let value = unsafe { CFRetain(kCFBooleanTrue).cast_mut() };
    // SAFETY: the closure writes the retained CF object owned by this transfer.
    assert!(
        unsafe {
            read_string(|output, _| {
                *output = value;
                0
            })
        }
        .is_none()
    );
}
