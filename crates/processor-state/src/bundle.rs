//! Portable, path-free framing for validated processor-state blobs.

use std::io::{self, Read, Write};

use crate::{
    StateBlobSink, StateBlobSource, StateError, StateRef,
    payload::{hex_digest, read_retry, verify_payload_stream},
};

const MAGIC: &[u8; 8] = b"MRPSTBND";
const VERSION: u16 = 1;

/// Maximum serialized manifest size accepted by a processor-state bundle.
pub const MAX_BUNDLE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of state references in a processor-state bundle.
pub const MAX_BUNDLE_REFERENCES: usize = 4096;
/// Maximum combined state payload size in a processor-state bundle.
pub const MAX_BUNDLE_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;

/// Writes a portable bundle containing a manifest and sorted, unique state payloads.
///
/// The framing uses a fixed magic, version, bounded little-endian lengths, raw SHA-256
/// digests, and payload bytes; it contains no filesystem paths or extraction instructions.
/// Payloads are verified before any archive bytes are written, then streamed and verified
/// again while copying.
///
/// `references` must be strictly sorted by digest; `source` holds every payload.
///
/// # Errors
/// Returns [`StateError`] for invalid manifest or reference bounds, duplicate or unsorted
/// references, missing/corrupt source payloads, or reader/writer failures.
pub fn write_bundle(
    mut writer: impl Write,
    manifest: &[u8],
    references: &[StateRef],
    source: &(impl StateBlobSource + ?Sized),
) -> Result<(), StateError> {
    let total_payload = validate_manifest_and_references(manifest, references)?;
    if total_payload > MAX_BUNDLE_PAYLOAD_BYTES {
        return Err(StateError::InvalidBundle {
            reason: "combined payload exceeds the maximum bundle size",
        });
    }
    for reference in references {
        source.verify(reference)?;
    }
    let manifest_length = u32::try_from(manifest.len()).map_err(|_| StateError::InvalidBundle {
        reason: "manifest length does not fit bundle framing",
    })?;
    let reference_count =
        u32::try_from(references.len()).map_err(|_| StateError::InvalidBundle {
            reason: "reference count does not fit bundle framing",
        })?;

    writer.write_all(MAGIC)?;
    writer.write_all(&VERSION.to_le_bytes())?;
    writer.write_all(&manifest_length.to_le_bytes())?;
    writer.write_all(&reference_count.to_le_bytes())?;
    writer.write_all(manifest)?;
    for reference in references {
        writer.write_all(&digest_bytes(reference)?)?;
        writer.write_all(&reference.byte_length().to_le_bytes())?;
        source.copy_verified(reference, &mut writer)?;
    }
    Ok(())
}

/// Reads a bounded bundle and keeps each payload in `sink` as soon as it verifies.
///
/// Payloads kept before a later frame or the trailing-byte check fails stay in `sink`; they
/// are intact, content-addressed and harmless.
///
/// # Errors
/// Returns [`StateError`] for unsupported framing, oversized fields, duplicate or unsorted
/// references, truncated/corrupt payloads, trailing bytes, or reader/sink failures.
pub fn read_bundle(
    reader: impl Read,
    sink: &(impl StateBlobSink + ?Sized),
) -> Result<(Vec<u8>, Vec<StateRef>), StateError> {
    read_frames(reader, |payload, reference| {
        sink.store_verified(payload, reference)
    })
}

/// Reads a bounded bundle and verifies every payload without keeping any.
///
/// # Errors
/// Returns the same errors as [`read_bundle`], apart from sink failures.
pub fn preview_bundle(reader: impl Read) -> Result<(Vec<u8>, Vec<StateRef>), StateError> {
    read_frames(reader, |payload, reference| {
        verify_payload_stream(payload, reference)
    })
}

fn read_frames(
    mut reader: impl Read,
    mut payload: impl FnMut(&mut dyn Read, &StateRef) -> Result<(), StateError>,
) -> Result<(Vec<u8>, Vec<StateRef>), StateError> {
    let mut magic = [0_u8; 8];
    read_framing(&mut reader, &mut magic)?;
    if &magic != MAGIC {
        return Err(StateError::InvalidBundle {
            reason: "unrecognized bundle magic",
        });
    }
    let version = read_u16(&mut reader)?;
    if version != VERSION {
        return Err(StateError::InvalidBundle {
            reason: "unsupported bundle version",
        });
    }
    let manifest_length = read_u32(&mut reader)? as usize;
    let reference_count = read_u32(&mut reader)? as usize;
    if manifest_length > MAX_BUNDLE_MANIFEST_BYTES {
        return Err(StateError::InvalidBundle {
            reason: "manifest exceeds the maximum byte length",
        });
    }
    if reference_count > MAX_BUNDLE_REFERENCES {
        return Err(StateError::InvalidBundle {
            reason: "reference count exceeds the maximum",
        });
    }

    let mut manifest = Vec::new();
    manifest
        .try_reserve_exact(manifest_length)
        .map_err(|_| StateError::Allocation)?;
    manifest.resize(manifest_length, 0);
    read_framing(&mut reader, &mut manifest)?;

    let mut references = Vec::new();
    references
        .try_reserve_exact(reference_count)
        .map_err(|_| StateError::Allocation)?;
    let mut previous_digest: Option<String> = None;
    let mut total_payload = 0_u64;
    for _ in 0..reference_count {
        let mut digest = [0_u8; 32];
        read_framing(&mut reader, &mut digest)?;
        let byte_length = read_u64(&mut reader)?;
        let reference = StateRef::new(hex_digest(&digest), byte_length)?;
        require_next_digest(previous_digest.as_deref(), &reference)?;
        previous_digest = Some(reference.sha256().to_owned());
        total_payload = total_payload
            .checked_add(byte_length)
            .filter(|total| *total <= MAX_BUNDLE_PAYLOAD_BYTES)
            .ok_or(StateError::InvalidBundle {
                reason: "combined payload exceeds the maximum bundle size",
            })?;

        // The handler sees only this payload, and must read all of it, so the next frame
        // header starts where the framing expects it.
        let mut bounded = (&mut reader).take(byte_length);
        payload(&mut bounded, &reference)?;
        if bounded.limit() != 0 {
            return Err(StateError::InvalidBundle {
                reason: "a payload was not read to its declared length",
            });
        }
        references.push(reference);
    }
    let mut trailing = [0_u8; 1];
    if read_retry(&mut reader, &mut trailing)? != 0 {
        return Err(StateError::InvalidBundle {
            reason: "trailing bytes after the final payload",
        });
    }
    Ok((manifest, references))
}

fn validate_manifest_and_references(
    manifest: &[u8],
    references: &[StateRef],
) -> Result<u64, StateError> {
    if manifest.len() > MAX_BUNDLE_MANIFEST_BYTES {
        return Err(StateError::InvalidBundle {
            reason: "manifest exceeds the maximum byte length",
        });
    }
    if references.len() > MAX_BUNDLE_REFERENCES {
        return Err(StateError::InvalidBundle {
            reason: "reference count exceeds the maximum",
        });
    }
    let mut previous_digest: Option<&str> = None;
    let mut total_payload = 0_u64;
    for reference in references {
        reference.validate()?;
        require_next_digest(previous_digest, reference)?;
        previous_digest = Some(reference.sha256());
        total_payload = total_payload
            .checked_add(reference.byte_length())
            .filter(|total| *total <= MAX_BUNDLE_PAYLOAD_BYTES)
            .ok_or(StateError::InvalidBundle {
                reason: "combined payload exceeds the maximum bundle size",
            })?;
    }
    Ok(total_payload)
}

fn require_next_digest(previous: Option<&str>, next: &StateRef) -> Result<(), StateError> {
    if previous.is_some_and(|previous| previous >= next.sha256()) {
        return Err(StateError::InvalidBundle {
            reason: "references must be strictly sorted by digest and contain no duplicates",
        });
    }
    Ok(())
}

fn digest_bytes(reference: &StateRef) -> Result<[u8; 32], StateError> {
    reference.validate()?;
    let mut bytes = [0_u8; 32];
    for (index, pair) in reference.sha256().as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8, StateError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(StateError::InvalidReference {
            reason: "digest must be 64 lowercase hexadecimal characters",
        }),
    }
}

fn read_u16(reader: &mut impl Read) -> Result<u16, StateError> {
    let mut bytes = [0_u8; 2];
    read_framing(reader, &mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(reader: &mut impl Read) -> Result<u32, StateError> {
    let mut bytes = [0_u8; 4];
    read_framing(reader, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> Result<u64, StateError> {
    let mut bytes = [0_u8; 8];
    read_framing(reader, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_framing(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), StateError> {
    match reader.read_exact(bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            Err(StateError::InvalidBundle {
                reason: "truncated bundle framing",
            })
        }
        Err(error) => Err(StateError::Io(error)),
    }
}

#[cfg(test)]
mod tests;
