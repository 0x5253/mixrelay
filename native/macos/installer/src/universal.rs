//! Bounded Universal 2 header inspection without requiring Xcode or `lipo`.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

use thiserror::Error;

const X86_64: u32 = 0x0100_0007;
const ARM64: u32 = 0x0100_000c;

/// A rejected Universal 2 executable or failed binary read.
#[derive(Debug, Error)]
pub enum UniversalError {
    /// The wrapper is not exactly one arm64 and one `x86_64` slice.
    #[error("expected a Universal 2 Mach-O with one arm64 and one x86_64 slice")]
    Architectures,
    /// Slice bounds, alignment or inner Mach-O identity are invalid.
    #[error("invalid Universal 2 slice bounds or Mach-O header")]
    Structure,
    /// The executable could not be read.
    #[error("could not inspect Universal 2 executable: {0}")]
    Io(#[from] io::Error),
}

/// Verifies the two architecture headers and their disjoint in-file ranges.
///
/// Reads a fixed-size table and two inner headers, not the whole executable.
/// This is a format check, not a signature check or path-ownership check.
/// # Errors
/// Returns missing/extra architectures, malformed ranges/headers, or read failure.
pub fn validate_universal_binary(path: &Path) -> Result<(), UniversalError> {
    let mut file = open_binary(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(UniversalError::Structure);
    }
    let length = metadata.len();
    validate(&mut file, length)
}

#[cfg(unix)]
fn open_binary(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    Ok(File::from(rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

#[cfg(not(unix))]
fn open_binary(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[derive(Clone, Copy)]
struct Slice {
    cpu: u32,
    offset: u64,
    end: u64,
}

fn validate(reader: &mut (impl Read + Seek), length: u64) -> Result<(), UniversalError> {
    let entry_size = match u32::from_be_bytes(read_array(reader)?) {
        0xcafe_babe => 20,
        0xcafe_babf => 32,
        _ => return Err(UniversalError::Architectures),
    };
    if u32::from_be_bytes(read_array(reader)?) != 2 {
        return Err(UniversalError::Architectures);
    }
    let first = read_slice(reader, entry_size, length)?;
    let second = read_slice(reader, entry_size, length)?;
    if !matches!((first.cpu, second.cpu), (ARM64, X86_64) | (X86_64, ARM64)) {
        return Err(UniversalError::Architectures);
    }
    if first.offset < second.end && second.offset < first.end {
        return Err(UniversalError::Structure);
    }
    for slice in [first, second] {
        reader.seek(SeekFrom::Start(slice.offset))?;
        let magic = u32::from_le_bytes(read_array(reader)?);
        let cpu = u32::from_le_bytes(read_array(reader)?);
        if magic != 0xfeed_facf || cpu != slice.cpu {
            return Err(UniversalError::Structure);
        }
    }
    Ok(())
}

fn read_slice(
    reader: &mut impl Read,
    entry_size: usize,
    length: u64,
) -> Result<Slice, UniversalError> {
    let cpu = u32::from_be_bytes(read_array(reader)?);
    let _subtype: [u8; 4] = read_array(reader)?;
    let (offset, size, alignment) = if entry_size == 20 {
        let offset = u32::from_be_bytes(read_array(reader)?);
        let size = u32::from_be_bytes(read_array(reader)?);
        let alignment = u32::from_be_bytes(read_array(reader)?);
        (u64::from(offset), u64::from(size), alignment)
    } else {
        let offset = u64::from_be_bytes(read_array(reader)?);
        let size = u64::from_be_bytes(read_array(reader)?);
        let alignment = u32::from_be_bytes(read_array(reader)?);
        let reserved = u32::from_be_bytes(read_array(reader)?);
        if reserved != 0 {
            return Err(UniversalError::Structure);
        }
        (offset, size, alignment)
    };
    let end = offset.checked_add(size).ok_or(UniversalError::Structure)?;
    let minimum_offset = if entry_size == 20 { 48 } else { 72 };
    if size < 32
        || offset < minimum_offset
        || end > length
        || alignment >= 63
        || offset % (1_u64 << alignment) != 0
    {
        return Err(UniversalError::Structure);
    }
    Ok(Slice { cpu, offset, end })
}

/// Reads the next fixed-width header field.
fn read_array<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests;
