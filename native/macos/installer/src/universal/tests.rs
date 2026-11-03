use super::*;
use std::io::Cursor;

pub(crate) fn fixture(wide: bool) -> Vec<u8> {
    let mut bytes = vec![0; 160];
    bytes[..4].copy_from_slice(&(if wide { 0xcafe_babf_u32 } else { 0xcafe_babe }).to_be_bytes());
    bytes[4..8].copy_from_slice(&2_u32.to_be_bytes());
    let stride = if wide { 32 } else { 20 };
    for (index, cpu) in [ARM64, X86_64].into_iter().enumerate() {
        let base = 8 + index * stride;
        let offset = 96 + u32::try_from(index).unwrap() * 32;
        bytes[base..base + 4].copy_from_slice(&cpu.to_be_bytes());
        if wide {
            bytes[base + 8..base + 16].copy_from_slice(&u64::from(offset).to_be_bytes());
            bytes[base + 16..base + 24].copy_from_slice(&32_u64.to_be_bytes());
            bytes[base + 24..base + 28].copy_from_slice(&5_u32.to_be_bytes());
        } else {
            bytes[base + 8..base + 12].copy_from_slice(&offset.to_be_bytes());
            bytes[base + 12..base + 16].copy_from_slice(&32_u32.to_be_bytes());
            bytes[base + 16..base + 20].copy_from_slice(&5_u32.to_be_bytes());
        }
        let offset = usize::try_from(offset).unwrap();
        bytes[offset..offset + 4].copy_from_slice(&0xfeed_facf_u32.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&cpu.to_le_bytes());
    }
    bytes
}

fn inspect(bytes: Vec<u8>) -> Result<(), UniversalError> {
    let length = bytes.len() as u64;
    validate(&mut Cursor::new(bytes), length)
}

#[test]
fn accepts_32_and_64_bit_fat_tables() {
    inspect(fixture(false)).unwrap();
    inspect(fixture(true)).unwrap();
}

#[test]
fn rejects_thin_duplicate_missing_and_unbounded_architectures() {
    for (offset, word) in [(0, 0xfeed_facf), (4, 1), (4, 3), (4, u32::MAX), (28, ARM64)] {
        let mut bytes = fixture(false);
        bytes[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
        assert!(inspect(bytes).is_err());
    }
}

#[test]
fn rejects_outside_overlapping_unaligned_and_overflowing_ranges() {
    for (offset, word) in [(16, 0), (16, 97), (20, 0), (20, 1000), (24, 64), (36, 96)] {
        let mut bytes = fixture(false);
        bytes[offset..offset + 4].copy_from_slice(&u32::try_from(word).unwrap().to_be_bytes());
        assert!(inspect(bytes).is_err());
    }
    let mut bytes = fixture(true);
    bytes[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(inspect(bytes).is_err());
}

#[test]
fn rejects_wrong_inner_magic_cpu_reserved_bits_and_truncation() {
    for offset in [36, 96, 100, 128, 132] {
        let mut bytes = fixture(true);
        bytes[offset] ^= 1;
        assert!(inspect(bytes).is_err());
    }
    for length in [0, 7, 47, 71, 127, 159] {
        let mut bytes = fixture(true);
        bytes.truncate(length);
        assert!(inspect(bytes).is_err());
    }
}
