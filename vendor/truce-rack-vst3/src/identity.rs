//! Encode native class identifiers and metadata strings.

use vst3::Steinberg::TUID;

pub(super) fn char8_array_to_string(array: &[vst3::Steinberg::char8]) -> String {
    // `char8` is signed on some targets and unsigned on others. Its native
    // representation is one byte, so this preserves the original UTF-8 bytes.
    let bytes: Vec<u8> = array
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&byte| byte.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(super) fn tuid_to_hex(cid: &TUID) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(32);
    for &byte in cid {
        let _ = write!(s, "{:02x}", byte.to_ne_bytes()[0]);
    }
    s
}

pub(super) fn hex_to_tuid(hex: &str) -> Option<TUID> {
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let mut out: TUID = [0; 16];
    for i in 0..16 {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        out[i] = std::ffi::c_char::from_ne_bytes([byte]);
    }
    Some(out)
}

#[cfg(test)]
mod tests;
