//! Strict RF64 fixture inspection. PCM decoding is delegated to Hound after
//! validating ds64 and translating the small fixture's container to RIFF.
use std::{
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::Path,
};

/// Sizes read from an RF64 file, without loading its audio data into memory.
#[derive(Debug, PartialEq, Eq)]
pub struct Metadata {
    pub riff_bytes: u64,
    pub data_bytes: u64,
    pub frames: u64,
    pub data_offset: u64,
}

/// Read and validate a finalized RF64 fixture, including its complete frames.
/// # Errors
/// Returns an error for malformed headers, sizes, frame counts or I/O failures.
pub fn inspect(path: impl AsRef<Path>) -> io::Result<Metadata> {
    let file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    inspect_stream(file, len)
}

/// Validate an in-memory RF64 fixture with the same size and frame checks.
/// # Errors
/// Returns an error for malformed headers, sizes or frame counts.
pub fn inspect_bytes(bytes: &[u8]) -> io::Result<Metadata> {
    inspect_stream(Cursor::new(bytes), bytes.len() as u64)
}

fn inspect_stream(mut file: impl Read + Seek, len: u64) -> io::Result<Metadata> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid RF64 fixture");
    let mut header = [0; 48];
    file.read_exact(&mut header)?;
    if &header[..4] != b"RF64"
        || header[4..8] != [255; 4]
        || &header[8..12] != b"WAVE"
        || &header[12..16] != b"ds64"
        || le_u32(&header[16..20])? != 28
        || header[44..48] != [0; 4]
    {
        return Err(invalid());
    }
    let riff_bytes = le_u64(&header[20..28])?;
    let data_bytes = le_u64(&header[28..36])?;
    let frames = le_u64(&header[36..44])?;
    if riff_bytes.checked_add(8) != Some(len) {
        return Err(invalid());
    }
    let mut block_align = 0;
    loop {
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk)?;
        let size = le_u32(&chunk[4..8])?;
        let start = file.stream_position()?;
        if &chunk[..4] == b"data" {
            if size != u32::MAX
                || block_align == 0
                || !data_bytes.is_multiple_of(block_align)
                || frames != data_bytes / block_align
                || start
                    .checked_add(data_bytes)
                    .and_then(|end| end.checked_add(data_bytes % 2))
                    != Some(len)
            {
                return Err(invalid());
            }
            return Ok(Metadata {
                riff_bytes,
                data_bytes,
                frames,
                data_offset: start,
            });
        }
        if &chunk[..4] == b"fmt " {
            if size < 16 {
                return Err(invalid());
            }
            let mut fmt = [0; 16];
            file.read_exact(&mut fmt)?;
            block_align = u64::from(u16::from_le_bytes([fmt[12], fmt[13]]));
        }
        let end = start
            .checked_add(u64::from(size))
            .and_then(|end| end.checked_add(u64::from(size % 2)))
            .filter(|end| *end <= len)
            .ok_or_else(invalid)?;
        file.seek(SeekFrom::Start(end))?;
    }
}

/// Decode a small RF64 fixture's samples with Hound after strict size validation.
/// # Errors
/// Rejects malformed RF64, unsupported PCM and fixtures larger than 16 MiB.
pub fn open(path: impl AsRef<Path>) -> Result<hound::WavReader<Cursor<Vec<u8>>>, hound::Error> {
    let metadata = inspect(&path)?;
    if metadata.riff_bytes > 16 * 1024 * 1024 {
        return Err(hound::Error::Unsupported);
    }
    decode(fs::read(path)?, &metadata)
}

/// Decode an in-memory RF64 fixture after strict size validation.
/// # Errors
/// Rejects malformed RF64, unsupported PCM and fixtures larger than 16 MiB.
pub fn open_bytes(bytes: Vec<u8>) -> Result<hound::WavReader<Cursor<Vec<u8>>>, hound::Error> {
    let metadata = inspect_bytes(&bytes)?;
    if metadata.riff_bytes > 16 * 1024 * 1024 {
        return Err(hound::Error::Unsupported);
    }
    decode(bytes, &metadata)
}

fn decode(
    mut bytes: Vec<u8>,
    metadata: &Metadata,
) -> Result<hound::WavReader<Cursor<Vec<u8>>>, hound::Error> {
    bytes[..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(
        &u32::try_from(metadata.riff_bytes)
            .map_err(|_| hound::Error::Unsupported)?
            .to_le_bytes(),
    );
    bytes[12..16].copy_from_slice(b"JUNK");
    let offset = usize::try_from(metadata.data_offset).map_err(|_| hound::Error::Unsupported)? - 4;
    bytes[offset..offset + 4].copy_from_slice(
        &u32::try_from(metadata.data_bytes)
            .map_err(|_| hound::Error::Unsupported)?
            .to_le_bytes(),
    );
    hound::WavReader::new(Cursor::new(bytes))
}

fn le_u32(bytes: &[u8]) -> io::Result<u32> {
    let word = bytes
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid RF64 word"))?;
    Ok(u32::from_le_bytes(word))
}

fn le_u64(bytes: &[u8]) -> io::Result<u64> {
    let word = bytes
        .try_into()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid RF64 word"))?;
    Ok(u64::from_le_bytes(word))
}
