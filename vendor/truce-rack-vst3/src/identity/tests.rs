//! Behavioral fixtures for the adjacent native owner.

use super::*;

#[test]
fn tuid_hex_roundtrip() {
    let cid: TUID = [
        0x01u8, 0x23, 0x45, 0x67, 0xf0, 0xf0, 0xf0, 0xf0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]
    .map(|byte| std::ffi::c_char::from_ne_bytes([byte]));
    let hex = tuid_to_hex(&cid);
    let parsed = hex_to_tuid(&hex).expect("parse");
    assert_eq!(parsed, cid);
}

#[test]
fn tuid_hex_keeps_two_digits_for_high_bytes() {
    let cid: TUID = [
        0x00u8, 0x01, 0x0f, 0x10, 0x7f, 0x80, 0x81, 0xfe, 0xff, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08,
    ]
    .map(|byte| std::ffi::c_char::from_ne_bytes([byte]));

    let hex = tuid_to_hex(&cid);

    assert_eq!(hex, "00010f107f8081feff02030405060708");
    assert_eq!(hex.len(), 32);
    assert_eq!(hex_to_tuid(&hex), Some(cid));
}

#[test]
fn tuid_hex_rejects_non_ascii_without_slicing_utf8() {
    let malformed = format!("aé{}", "0".repeat(29));
    assert_eq!(malformed.len(), 32);
    assert_eq!(hex_to_tuid(&malformed), None);
}

#[test]
fn char8_skips_trailing_nul() {
    let mut arr = [0 as vst3::Steinberg::char8; 16];
    for (i, &b) in b"Hello".iter().enumerate() {
        arr[i] = vst3::Steinberg::char8::from_ne_bytes([b]);
    }
    assert_eq!(char8_array_to_string(&arr), "Hello");
}

#[test]
fn char8_preserves_non_ascii_utf8_on_both_c_abis() {
    let bytes: Vec<vst3::Steinberg::char8> = "마이크\0ignored"
        .as_bytes()
        .iter()
        .map(|&byte| vst3::Steinberg::char8::from_ne_bytes([byte]))
        .collect();
    assert_eq!(char8_array_to_string(&bytes), "마이크");
}
