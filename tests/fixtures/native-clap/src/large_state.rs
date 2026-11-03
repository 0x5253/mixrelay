//! Deterministic native state bytes for the explicitly selected large fixture instance.

use clap_sys::stream::{clap_istream, clap_ostream};

use super::{STATE_SIZE, read_stream_exact, write_stream_all};

const STATE_BYTES: usize = 32 * 1024 * 1024;
const CHUNK_BYTES: usize = 8 * 1024;

fn expected_byte(index: usize) -> u8 {
    (index.wrapping_mul(131) ^ (index >> 9) ^ (index >> 19)).to_le_bytes()[0]
}

/// Writes the bytes after the ordinary parameter header without retaining a large buffer.
///
/// # Safety
/// `stream` must be the live CLAP output stream passed to `state.save`.
pub(super) unsafe fn write_payload(stream: *const clap_ostream) -> bool {
    let mut chunk = [0_u8; CHUNK_BYTES];
    let mut offset = STATE_SIZE;
    while offset < STATE_BYTES {
        let length = (STATE_BYTES - offset).min(CHUNK_BYTES);
        for (index, byte) in chunk[..length].iter_mut().enumerate() {
            *byte = expected_byte(offset + index);
        }
        // SAFETY: the host owns this stream until the state.save callback returns.
        if !unsafe { write_stream_all(stream, &chunk[..length]) } {
            return false;
        }
        offset += length;
    }
    true
}

/// Checks every native payload byte before the caller applies the decoded parameters.
///
/// # Safety
/// `stream` must be the live CLAP input stream passed to `state.load`.
pub(super) unsafe fn validate_payload(stream: *const clap_istream) -> bool {
    let mut chunk = [0_u8; CHUNK_BYTES];
    let mut offset = STATE_SIZE;
    while offset < STATE_BYTES {
        let length = (STATE_BYTES - offset).min(CHUNK_BYTES);
        // SAFETY: the host owns this stream until the state.load callback returns.
        if !unsafe { read_stream_exact(stream, &mut chunk[..length]) } {
            return false;
        }
        if chunk[..length]
            .iter()
            .enumerate()
            .any(|(index, byte)| *byte != expected_byte(offset + index))
        {
            return false;
        }
        offset += length;
    }
    true
}

#[cfg(test)]
mod tests {
    use std::{ffi::c_void, ptr};

    use clap_sys::stream::{clap_istream, clap_ostream};

    use super::*;
    use crate::{ParameterValues, decode_native_state};

    struct Output {
        bytes: Vec<u8>,
        max_transfer: usize,
        stop_after: Option<usize>,
        error_on_stop: bool,
    }

    unsafe extern "C" fn write(stream: *const clap_ostream, data: *const c_void, size: u64) -> i64 {
        // SAFETY: each test passes a live Output as ctx and a readable CLAP buffer.
        let output = unsafe { &mut *(*stream).ctx.cast::<Output>() };
        if output
            .stop_after
            .is_some_and(|limit| output.bytes.len() >= limit)
        {
            return if output.error_on_stop { -1 } else { 0 };
        }
        let requested = usize::try_from(size).expect("bounded fixture stream request");
        let count = requested.min(output.max_transfer);
        // SAFETY: the fixture advertised this many readable bytes to the callback.
        output
            .bytes
            .extend_from_slice(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), count) });
        i64::try_from(count).expect("bounded fixture stream response")
    }

    impl Output {
        fn new(max_transfer: usize) -> Self {
            Self {
                bytes: Vec::new(),
                max_transfer,
                stop_after: None,
                error_on_stop: false,
            }
        }

        fn stream(&mut self) -> clap_ostream {
            clap_ostream {
                ctx: ptr::from_mut(self).cast(),
                write: Some(write),
            }
        }
    }

    struct Input<'a> {
        bytes: &'a [u8],
        offset: usize,
        max_transfer: usize,
        error_after: Option<usize>,
    }

    unsafe extern "C" fn read(stream: *const clap_istream, data: *mut c_void, size: u64) -> i64 {
        // SAFETY: each test passes a live Input as ctx and a writable CLAP buffer.
        let input = unsafe { &mut *(*stream).ctx.cast::<Input<'_>>() };
        if input.error_after.is_some_and(|limit| input.offset >= limit) {
            return -1;
        }
        let requested = usize::try_from(size).expect("bounded fixture stream request");
        let count = requested
            .min(input.max_transfer)
            .min(input.bytes.len() - input.offset);
        // SAFETY: both spans contain count bytes; ctx keeps the source alive through this call.
        unsafe {
            ptr::copy_nonoverlapping(input.bytes.as_ptr().add(input.offset), data.cast(), count);
        }
        input.offset += count;
        i64::try_from(count).expect("bounded fixture stream response")
    }

    impl<'a> Input<'a> {
        fn new(bytes: &'a [u8], max_transfer: usize) -> Self {
            Self {
                bytes,
                offset: 0,
                max_transfer,
                error_after: None,
            }
        }

        fn stream(&mut self) -> clap_istream {
            clap_istream {
                ctx: ptr::from_mut(self).cast(),
                read: Some(read),
            }
        }
    }

    fn saved_bytes(max_transfer: usize) -> Vec<u8> {
        let mut output = Output::new(max_transfer);
        let stream = output.stream();
        let header = ParameterValues {
            gain_a: 0.5,
            gain_b: 0.25,
            delay_frames: 64,
        }
        .encode();
        // SAFETY: the mock stream and its context remain live across both writes.
        assert!(unsafe { write_stream_all(&raw const stream, &header) });
        // SAFETY: the mock stream and its context remain live across payload writes.
        assert!(unsafe { write_payload(&raw const stream) });
        output.bytes
    }

    #[test]
    fn short_positive_stream_transfers_preserve_every_byte() {
        let bytes = saved_bytes(4093);
        assert_eq!(bytes.len(), STATE_BYTES);
        assert_eq!(bytes[STATE_SIZE], expected_byte(STATE_SIZE));
        assert_eq!(bytes[STATE_BYTES - 1], expected_byte(STATE_BYTES - 1));

        let mut input = Input::new(&bytes, 2053);
        let stream = input.stream();
        // SAFETY: the mock stream and bytes remain live through decode.
        let values =
            unsafe { decode_native_state(&raw const stream, true) }.expect("valid large state");
        assert_eq!(values.gain_a.to_bits(), 0.5_f64.to_bits());
        assert_eq!(values.gain_b.to_bits(), 0.25_f64.to_bits());
        assert_eq!(values.delay_frames, 64);
        assert_eq!(input.offset, STATE_BYTES);
    }

    #[test]
    fn corrupt_or_truncated_payload_is_rejected_before_parameters_are_returned() {
        let mut bytes = saved_bytes(CHUNK_BYTES);
        let corrupt_at = STATE_BYTES - 19;
        bytes[corrupt_at] ^= 1;
        let mut corrupt = Input::new(&bytes, CHUNK_BYTES);
        let stream = corrupt.stream();
        // SAFETY: the mock stream and bytes remain live through decode.
        assert!(unsafe { decode_native_state(&raw const stream, true) }.is_none());

        bytes[corrupt_at] ^= 1;
        let mut truncated = Input::new(&bytes[..STATE_BYTES - 1], CHUNK_BYTES);
        let stream = truncated.stream();
        // SAFETY: the mock stream and bytes remain live through decode.
        assert!(unsafe { decode_native_state(&raw const stream, true) }.is_none());

        let mut failed = Input::new(&bytes, CHUNK_BYTES);
        failed.error_after = Some(CHUNK_BYTES);
        let stream = failed.stream();
        // SAFETY: the mock stream and bytes remain live through decode.
        assert!(unsafe { decode_native_state(&raw const stream, true) }.is_none());
    }

    #[test]
    fn zero_or_error_write_rejects_incomplete_large_state() {
        for error_on_stop in [false, true] {
            let mut output = Output::new(4093);
            output.stop_after = Some(STATE_SIZE + CHUNK_BYTES);
            output.error_on_stop = error_on_stop;
            let stream = output.stream();
            let header = ParameterValues {
                gain_a: 0.5,
                gain_b: 0.25,
                delay_frames: 64,
            }
            .encode();
            // SAFETY: the mock stream and its context remain live across both writes.
            assert!(unsafe { write_stream_all(&raw const stream, &header) });
            // SAFETY: the mock stream and its context remain live across payload writes.
            assert!(!unsafe { write_payload(&raw const stream) });
            assert!(output.bytes.len() < STATE_BYTES);
        }
    }
}
