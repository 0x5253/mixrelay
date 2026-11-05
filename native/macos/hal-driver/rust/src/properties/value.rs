//! Property payloads and how each kind is written into host storage.
use super::status::{BAD_SIZE, ILLEGAL};
use std::ffi::c_void;

pub(super) const UTF8: u32 = 0x0800_0100;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const c_void,
        bytes: *const u8,
        count: isize,
        encoding: u32,
        external: u8,
    ) -> *const c_void;
}

/// Largest fixed payload stored inline: an `AudioValueRange` (two `Float64`).
const INLINE_BYTES: usize = 16;

/// A fixed payload of at most `INLINE_BYTES`, stored without allocation. Scalars use it,
/// so the answers given on IO threads never allocate.
#[derive(Clone, Copy)]
pub(super) struct Inline {
    bytes: [u8; INLINE_BYTES],
    length: usize,
}

impl Inline {
    pub(super) fn new<const N: usize>(value: [u8; N]) -> Self {
        const { assert!(N <= INLINE_BYTES, "inline payload exceeds INLINE_BYTES") };
        let mut bytes = [0; INLINE_BYTES];
        let (head, _) = bytes.split_at_mut(N);
        head.copy_from_slice(&value);
        Self { bytes, length: N }
    }

    fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }
}

pub(super) enum Value {
    /// A fixed scalar or small struct; never allocates.
    Inline(Inline),
    /// A fixed struct too large to store inline (ASBD, channel layout).
    Struct(Vec<u8>),
    /// Packed elements of `stride` bytes; a smaller output receives whole elements only.
    Array(Vec<u8>, usize),
    /// UTF-8 handed to the host as a new `CFString`.
    Text(Vec<u8>),
    /// Bytes handed to the host as a new `CFData`.
    Data(Vec<u8>),
}

impl Value {
    pub(super) fn size(&self) -> usize {
        match self {
            Self::Inline(inline) => inline.length,
            Self::Struct(bytes) | Self::Array(bytes, _) => bytes.len(),
            Self::Text(_) | Self::Data(_) => size_of::<*const c_void>(),
        }
    }

    /// # Safety
    /// A non-null `output` must be writable for `capacity` bytes for this call.
    pub(super) unsafe fn write(&self, capacity: usize, output: *mut c_void) -> Result<usize, i32> {
        match self {
            // SAFETY: forwarded from this function's contract.
            Self::Inline(inline) => unsafe { write_fixed(inline.as_bytes(), capacity, output) },
            // SAFETY: as above.
            Self::Struct(bytes) => unsafe { write_fixed(bytes, capacity, output) },
            Self::Array(bytes, stride) => {
                let size = bytes.len().min(capacity / stride * stride);
                if size > 0 {
                    // SAFETY: size is at most capacity, which the caller made writable.
                    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), size) };
                }
                Ok(size)
            }
            Self::Data(bytes) => {
                // SAFETY: forwarded caller storage; the returned CFData is host-owned.
                unsafe { crate::property_data::write(bytes, capacity, output) }
                    .map_err(super::status::data_error)
            }
            Self::Text(bytes) => {
                if capacity < size_of::<*const c_void>() {
                    return Err(BAD_SIZE);
                }
                let count = isize::try_from(bytes.len()).map_err(|_| BAD_SIZE)?;
                // SAFETY: bytes is a live slice of exactly count bytes.
                let string = unsafe {
                    CFStringCreateWithBytes(std::ptr::null(), bytes.as_ptr(), count, UTF8, 0)
                };
                if string.is_null() {
                    return Err(ILLEGAL);
                }
                // SAFETY: capacity holds one pointer; the SDK output need not be aligned.
                unsafe { output.cast::<*const c_void>().write_unaligned(string) };
                Ok(size_of::<*const c_void>())
            }
        }
    }
}

/// Writes a fixed payload whole, or refuses a smaller output.
///
/// # Safety
/// A non-null `output` must be writable for `capacity` bytes for this call.
unsafe fn write_fixed(bytes: &[u8], capacity: usize, output: *mut c_void) -> Result<usize, i32> {
    if capacity < bytes.len() || output.is_null() {
        return Err(BAD_SIZE);
    }
    // SAFETY: output is non-null and writable for capacity >= bytes.len() bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast(), bytes.len()) };
    Ok(bytes.len())
}

/// Two adjacent fields of one fixed SDK struct, in native byte order.
pub(super) fn pair<const N: usize, const M: usize>(first: [u8; N], second: [u8; N]) -> [u8; M] {
    const { assert!(M == 2 * N, "a pair holds exactly two fields") };
    let mut bytes = [0; M];
    let (head, tail) = bytes.split_at_mut(N);
    head.copy_from_slice(&first);
    tail.copy_from_slice(&second);
    bytes
}

pub(super) fn number(value: u32) -> Value {
    Value::Inline(Inline::new(value.to_ne_bytes()))
}

pub(super) fn code(value: [u8; 4]) -> Value {
    number(u32::from_be_bytes(value))
}

/// Text up to its first NUL.
pub(super) fn text(value: &[u8]) -> Value {
    Value::Text(
        value
            .split(|byte| *byte == 0)
            .next()
            .unwrap_or_default()
            .to_vec(),
    )
}

pub(super) fn objects(values: impl IntoIterator<Item = u32>) -> Value {
    Value::Array(values.into_iter().flat_map(u32::to_ne_bytes).collect(), 4)
}

pub(super) fn rate(value: u32) -> Value {
    Value::Inline(Inline::new(f64::from(value).to_ne_bytes()))
}

pub(super) fn scalar(value: f32) -> Value {
    Value::Inline(Inline::new(value.to_ne_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(value: &Value, capacity: usize) -> (Result<usize, i32>, Vec<u8>) {
        let mut output = vec![0xAA_u8; capacity];
        // SAFETY: output is writable for exactly capacity bytes.
        let result = unsafe { value.write(capacity, output.as_mut_ptr().cast()) };
        (result, output)
    }

    #[test]
    fn fixed_payloads_are_written_whole_or_refused() {
        let value = rate(48_000);
        assert_eq!(value.size(), 8);
        assert_eq!(
            written(&value, 16),
            (Ok(8), {
                let mut expected = 48_000.0_f64.to_ne_bytes().to_vec();
                expected.extend([0xAA; 8]);
                expected
            })
        );
        assert_eq!(written(&value, 7), (Err(BAD_SIZE), vec![0xAA; 7]));
        // SAFETY: a null output is rejected before any write.
        assert_eq!(
            unsafe { value.write(8, std::ptr::null_mut()) },
            Err(BAD_SIZE)
        );
    }

    #[test]
    fn arrays_fill_only_whole_elements_that_fit() {
        let value = objects([1, 2, 3]);
        assert_eq!(value.size(), 12);
        let (result, output) = written(&value, 11);
        assert_eq!(result, Ok(8));
        assert_eq!(
            output[..8],
            [1_u32.to_ne_bytes(), 2_u32.to_ne_bytes()].concat()
        );
        assert_eq!(output[8..], [0xAA; 3]);
        assert_eq!(written(&value, 3).0, Ok(0));
    }

    #[test]
    fn host_objects_report_one_pointer_and_need_room_for_it() {
        let value = text(b"MixRelay\0ignored");
        assert_eq!(value.size(), size_of::<*const c_void>());
        assert!(matches!(&value, Value::Text(bytes) if bytes == b"MixRelay"));
        assert_eq!(written(&value, 4).0, Err(BAD_SIZE));
    }
}
