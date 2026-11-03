//! Streaming SHA-256 verification shared by the store and bundle framing.
use std::io::{self, Read, Write};

use sha2::{Digest, Sha256};

use crate::{StateError, StateRef};

const IO_CHUNK_BYTES: usize = 64 * 1024;

pub(crate) fn verify_payload_stream(
    reader: &mut (impl Read + ?Sized),
    reference: &StateRef,
) -> Result<(), StateError> {
    reference.validate()?;
    hash_exact(
        reader,
        reference.byte_length(),
        None,
        Some(reference),
        false,
    )?;
    Ok(())
}

pub(crate) fn hash_exact(
    reader: &mut (impl Read + ?Sized),
    byte_length: u64,
    mut writer: Option<&mut dyn Write>,
    expected: Option<&StateRef>,
    check_eof: bool,
) -> Result<String, StateError> {
    let mut hasher = Sha256::new();
    let mut remaining = byte_length;
    let chunk_size = byte_length.min(IO_CHUNK_BYTES as u64);
    let buffer_size = usize::try_from(chunk_size).map_err(|_| StateError::Allocation)?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(buffer_size)
        .map_err(|_| StateError::Allocation)?;
    buffer.resize(buffer_size, 0_u8);
    while remaining > 0 {
        let wanted =
            usize::try_from(remaining.min(chunk_size)).map_err(|_| StateError::Allocation)?;
        let count = read_retry(reader, &mut buffer[..wanted])?;
        if count == 0 {
            return Err(StateError::Integrity);
        }
        hasher.update(&buffer[..count]);
        if let Some(output) = writer.as_deref_mut() {
            output.write_all(&buffer[..count])?;
        }
        remaining -= count as u64;
    }

    if check_eof {
        let mut extra = [0_u8; 1];
        if read_retry(reader, &mut extra)? != 0 {
            return Err(StateError::Integrity);
        }
    }
    let digest = hasher.finalize();
    let digest: [u8; 32] = digest.into();
    let encoded = hex_digest(&digest);
    if expected.is_some_and(|reference| encoded != reference.sha256()) {
        return Err(StateError::Integrity);
    }
    Ok(encoded)
}

pub(crate) fn read_retry(
    reader: &mut (impl Read + ?Sized),
    buffer: &mut [u8],
) -> io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

pub(crate) fn hex_digest(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[usize::from(*byte >> 4)] as char);
        encoded.push(HEX[usize::from(*byte & 0x0f)] as char);
    }
    encoded
}
