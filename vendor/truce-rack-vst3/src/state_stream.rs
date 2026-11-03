//! Bounded in-memory `IBStream` used for VST3 component state.

use std::cell::{Cell, RefCell};
use std::fmt;

use vst3::Class;
use vst3::Steinberg::{IBStream, IBStreamTrait, kInvalidArgument, kOutOfMemory, kResultOk};

/// Maximum component-state payload accepted by the processor-host transport.
pub(crate) const STATE_STREAM_CAPACITY: usize = 64 * 1024 * 1024;

const SEEK_SET: u32 = 0;
const SEEK_CUR: u32 = 1;
const SEEK_END: u32 = 2;

/// Failure categories retained by a state stream after a rejected operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamError {
    /// A caller supplied an invalid pointer, byte count, or seek mode.
    InvalidArgument,
    /// A seek would move the cursor outside the bounded stream.
    PositionOutOfRange,
    /// A write or input payload would exceed the state budget.
    CapacityExceeded,
    /// The backing vector could not reserve the requested bounded capacity.
    AllocationFailed,
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidArgument => "invalid argument",
            Self::PositionOutOfRange => "stream position is out of range",
            Self::CapacityExceeded => "state exceeds its stream capacity",
            Self::AllocationFailed => "state stream allocation failed",
        };
        formatter.write_str(message)
    }
}

/// Bounded storage for the `IBStream` handed to `IComponent::setState` and
/// `IComponent::getState`.
pub(crate) struct MemoryStream {
    data: RefCell<Vec<u8>>,
    position: Cell<usize>,
    failure: Cell<Option<StreamError>>,
    capacity: usize,
}

impl Default for MemoryStream {
    fn default() -> Self {
        Self {
            data: RefCell::new(Vec::new()),
            position: Cell::new(0),
            failure: Cell::new(None),
            capacity: STATE_STREAM_CAPACITY,
        }
    }
}

impl MemoryStream {
    /// Construct a read stream without allocating before the payload is known
    /// to fit the state budget.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, StreamError> {
        if bytes.len() > STATE_STREAM_CAPACITY {
            return Err(StreamError::CapacityExceeded);
        }
        let mut data = Vec::new();
        data.try_reserve_exact(bytes.len())
            .map_err(|_| StreamError::AllocationFailed)?;
        data.extend_from_slice(bytes);
        Ok(Self {
            data: RefCell::new(data),
            position: Cell::new(0),
            failure: Cell::new(None),
            capacity: STATE_STREAM_CAPACITY,
        })
    }

    /// Construct a write stream with a stricter byte ceiling than the transport maximum.
    pub(crate) fn with_capacity_limit(capacity: usize) -> Result<Self, StreamError> {
        if capacity > STATE_STREAM_CAPACITY {
            return Err(StreamError::CapacityExceeded);
        }
        Ok(Self {
            data: RefCell::new(Vec::new()),
            position: Cell::new(0),
            failure: Cell::new(None),
            capacity,
        })
    }

    /// Return the first stream failure observed by a COM callback, if any.
    pub(crate) fn stream_error(&self) -> Option<StreamError> {
        self.failure.get()
    }

    /// Copy the captured state after the caller has checked [`Self::stream_error`].
    pub(crate) fn to_vec(&self) -> Result<Vec<u8>, StreamError> {
        let data = self.data.borrow();
        let mut copy = Vec::new();
        copy.try_reserve_exact(data.len())
            .map_err(|_| self.record_failure(StreamError::AllocationFailed))?;
        copy.extend_from_slice(&data);
        Ok(copy)
    }

    fn record_failure(&self, error: StreamError) -> StreamError {
        if self.failure.get().is_none() {
            self.failure.set(Some(error));
        }
        error
    }

    fn read_at_cursor(&self, output: &mut [u8]) -> usize {
        let data = self.data.borrow();
        let position = self.position.get();
        let start = position.min(data.len());
        let available = data.len() - start;
        let count = output.len().min(available);
        output[..count].copy_from_slice(&data[start..start + count]);
        self.position.set(position + count);
        count
    }

    fn available_bytes(&self) -> usize {
        let data = self.data.borrow();
        data.len().saturating_sub(self.position.get())
    }

    fn write_end(&self, input_len: usize) -> Result<usize, StreamError> {
        let end = self
            .position
            .get()
            .checked_add(input_len)
            .ok_or(StreamError::PositionOutOfRange)?;
        if end > self.capacity {
            return Err(StreamError::CapacityExceeded);
        }
        Ok(end)
    }

    fn write_at_cursor(&self, input: &[u8]) -> Result<usize, StreamError> {
        let position = self.position.get();
        let end = self
            .write_end(input.len())
            .map_err(|error| self.record_failure(error))?;

        let mut data = self.data.borrow_mut();
        if end > data.len() {
            let additional = end - data.len();
            data.try_reserve_exact(additional)
                .map_err(|_| self.record_failure(StreamError::AllocationFailed))?;
            data.resize(end, 0);
        }
        data[position..end].copy_from_slice(input);
        self.position.set(end);
        Ok(input.len())
    }

    fn seek_to(&self, offset: i64, mode: u32) -> Result<i64, StreamError> {
        let base = match mode {
            SEEK_SET => 0,
            SEEK_CUR => self.position.get(),
            SEEK_END => self.data.borrow().len(),
            _ => return Err(self.record_failure(StreamError::InvalidArgument)),
        };
        let base = i64::try_from(base)
            .map_err(|_| self.record_failure(StreamError::PositionOutOfRange))?;
        let position = base
            .checked_add(offset)
            .ok_or_else(|| self.record_failure(StreamError::PositionOutOfRange))?;
        if position < 0 {
            return Err(self.record_failure(StreamError::PositionOutOfRange));
        }
        let capacity = i64::try_from(self.capacity)
            .map_err(|_| self.record_failure(StreamError::PositionOutOfRange))?;
        if position > capacity {
            return Err(self.record_failure(StreamError::CapacityExceeded));
        }
        let position = usize::try_from(position)
            .map_err(|_| self.record_failure(StreamError::PositionOutOfRange))?;
        self.position.set(position);
        Ok(i64::try_from(position).expect("bounded state cursor fits in i64"))
    }

    fn fail(&self, error: StreamError) -> i32 {
        match self.record_failure(error) {
            StreamError::AllocationFailed | StreamError::CapacityExceeded => kOutOfMemory,
            StreamError::InvalidArgument | StreamError::PositionOutOfRange => kInvalidArgument,
        }
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> i32 {
        if !num_bytes_read.is_null() {
            // SAFETY: The VST3 ABI provides an optional writable result pointer. It is only
            // dereferenced after the null check and is used for this synchronous callback.
            unsafe { *num_bytes_read = 0 };
        }
        if num_bytes < 0 || (num_bytes != 0 && buffer.is_null()) {
            return self.fail(StreamError::InvalidArgument);
        }
        let Ok(count) = usize::try_from(num_bytes) else {
            return self.fail(StreamError::InvalidArgument);
        };
        if count == 0 {
            return kResultOk;
        }
        let count = count.min(self.available_bytes());
        if count == 0 {
            return kResultOk;
        }

        // SAFETY: IBStream requires the plugin to provide a writable buffer containing at least
        // `num_bytes` bytes. The slice is borrowed only for the duration of this callback and is
        // never retained by the stream.
        let output = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), count) };
        let count = self.read_at_cursor(output);
        let Ok(count) = i32::try_from(count) else {
            return self.fail(StreamError::PositionOutOfRange);
        };
        if !num_bytes_read.is_null() {
            // SAFETY: The result pointer was checked above and `count <= num_bytes <= i32::MAX`.
            unsafe { *num_bytes_read = count };
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> i32 {
        if !num_bytes_written.is_null() {
            // SAFETY: The VST3 ABI provides an optional writable result pointer. It is only
            // dereferenced after the null check and is used for this synchronous callback.
            unsafe { *num_bytes_written = 0 };
        }
        if num_bytes < 0 || (num_bytes != 0 && buffer.is_null()) {
            return self.fail(StreamError::InvalidArgument);
        }
        let Ok(count) = usize::try_from(num_bytes) else {
            return self.fail(StreamError::InvalidArgument);
        };
        if count == 0 {
            return kResultOk;
        }
        if let Err(error) = self.write_end(count) {
            return self.fail(error);
        }

        // SAFETY: IBStream requires the plugin to provide a readable buffer containing at least
        // `num_bytes` bytes. The slice is borrowed only for the duration of this callback and is
        // never retained by the stream.
        let input = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), count) };
        match self.write_at_cursor(input) {
            Ok(_) => {}
            Err(error) => return self.fail(error),
        }
        if !num_bytes_written.is_null() {
            // SAFETY: The result pointer was checked above and `written == num_bytes`.
            unsafe { *num_bytes_written = num_bytes };
        }
        kResultOk
    }

    unsafe fn seek(&self, offset: i64, mode: i32, result: *mut i64) -> i32 {
        let mode = u32::try_from(mode).unwrap_or(u32::MAX);
        let position = match self.seek_to(offset, mode) {
            Ok(position) => position,
            Err(error) => return self.fail(error),
        };
        if !result.is_null() {
            // SAFETY: The VST3 ABI provides an optional writable result pointer. It is only
            // dereferenced after the null check and is used for this synchronous callback.
            unsafe { *result = position };
        }
        kResultOk
    }

    unsafe fn tell(&self, position: *mut i64) -> i32 {
        if position.is_null() {
            return self.fail(StreamError::InvalidArgument);
        }
        // SAFETY: The VST3 ABI requires a writable result pointer for `tell`; null was rejected
        // above, and the bounded cursor always fits in `i64`.
        unsafe {
            *position =
                i64::try_from(self.position.get()).expect("bounded state cursor fits in i64");
        };
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;

    #[test]
    fn write_stops_at_the_state_capacity_without_mutating_failed_call() {
        let stream = MemoryStream::default();
        let payload = vec![0x5a; STATE_STREAM_CAPACITY];
        assert_eq!(stream.write_at_cursor(&payload), Ok(STATE_STREAM_CAPACITY));
        assert_eq!(stream.position.get(), STATE_STREAM_CAPACITY);
        assert_eq!(
            stream.write_at_cursor(&[0x7f]),
            Err(StreamError::CapacityExceeded)
        );
        assert_eq!(stream.position.get(), STATE_STREAM_CAPACITY);
        assert_eq!(stream.data.borrow().len(), STATE_STREAM_CAPACITY);
        assert_eq!(stream.stream_error(), Some(StreamError::CapacityExceeded));
    }

    #[test]
    fn stricter_capacity_limit_bounds_writes_and_seeks() {
        let stream = MemoryStream::with_capacity_limit(3).expect("limit is under transport cap");
        assert_eq!(stream.write_at_cursor(b"abc"), Ok(3));
        assert_eq!(
            stream.write_at_cursor(b"d"),
            Err(StreamError::CapacityExceeded)
        );
        assert_eq!(stream.data.borrow().as_slice(), b"abc");
        assert_eq!(stream.position.get(), 3);
        assert_eq!(stream.stream_error(), Some(StreamError::CapacityExceeded));

        let seek_limited = MemoryStream::with_capacity_limit(3).expect("valid limit");
        assert_eq!(
            seek_limited.seek_to(4, SEEK_SET),
            Err(StreamError::CapacityExceeded)
        );
        assert_eq!(seek_limited.position.get(), 0);
    }

    #[test]
    fn stricter_capacity_rejects_values_above_transport_cap() {
        assert!(matches!(
            MemoryStream::with_capacity_limit(STATE_STREAM_CAPACITY + 1),
            Err(StreamError::CapacityExceeded)
        ));
        assert!(MemoryStream::with_capacity_limit(0).is_ok());
    }

    #[test]
    fn oversized_write_is_sticky_for_state_snapshot() {
        let stream = MemoryStream::default();
        let capacity = i64::try_from(STATE_STREAM_CAPACITY).expect("state capacity fits in i64");
        assert_eq!(stream.seek_to(capacity, SEEK_SET), Ok(capacity));
        assert_eq!(
            stream.write_at_cursor(&[1]),
            Err(StreamError::CapacityExceeded)
        );
        assert_eq!(stream.stream_error(), Some(StreamError::CapacityExceeded));

        // The COM callback records the failure; later save-state code must inspect this before
        // treating the captured bytes as a successful snapshot.
        let oversized_payload = vec![1u8; STATE_STREAM_CAPACITY + 1];
        let oversized_len =
            i32::try_from(oversized_payload.len()).expect("test payload fits in i32");
        let mut written = 123;
        let status = unsafe {
            IBStreamTrait::write(
                &stream,
                oversized_payload.as_ptr().cast_mut().cast::<c_void>(),
                oversized_len,
                &raw mut written,
            )
        };
        assert_eq!(status, kOutOfMemory);
        assert_eq!(written, 0);
        assert_eq!(stream.stream_error(), Some(StreamError::CapacityExceeded));
        assert_eq!(stream.to_vec(), Ok(Vec::new()));
    }

    #[test]
    fn invalid_read_and_write_arguments_zero_byte_counts() {
        let stream = MemoryStream::default();
        let mut read = 99;
        let read_status =
            unsafe { IBStreamTrait::read(&stream, std::ptr::null_mut(), 1, &raw mut read) };
        assert_eq!(read_status, kInvalidArgument);
        assert_eq!(read, 0);

        let mut written = 99;
        let write_status =
            unsafe { IBStreamTrait::write(&stream, std::ptr::null_mut(), -1, &raw mut written) };
        assert_eq!(write_status, kInvalidArgument);
        assert_eq!(written, 0);
        assert_eq!(stream.stream_error(), Some(StreamError::InvalidArgument));
    }

    #[test]
    fn zero_length_io_accepts_a_null_buffer() {
        let stream = MemoryStream::default();
        let mut read = 99;
        let read_status =
            unsafe { IBStreamTrait::read(&stream, std::ptr::null_mut(), 0, &raw mut read) };
        assert_eq!(read_status, kResultOk);
        assert_eq!(read, 0);

        let mut written = 99;
        let write_status =
            unsafe { IBStreamTrait::write(&stream, std::ptr::null_mut(), 0, &raw mut written) };
        assert_eq!(write_status, kResultOk);
        assert_eq!(written, 0);
        assert_eq!(stream.stream_error(), None);
    }

    #[test]
    fn seek_supports_set_current_and_end_modes() {
        let stream = MemoryStream::from_bytes(b"abcd").expect("small fixture fits");
        assert_eq!(stream.seek_to(1, SEEK_SET), Ok(1));
        assert_eq!(stream.seek_to(2, SEEK_CUR), Ok(3));
        assert_eq!(stream.seek_to(-1, SEEK_END), Ok(3));
        assert_eq!(stream.position.get(), 3);
        assert_eq!(stream.stream_error(), None);
    }

    #[test]
    fn seek_rejects_invalid_modes_underflow_overflow_and_capacity() {
        let stream = MemoryStream::default();
        let capacity = i64::try_from(STATE_STREAM_CAPACITY).expect("state capacity fits in i64");
        assert_eq!(stream.seek_to(0, 99), Err(StreamError::InvalidArgument));
        assert_eq!(
            stream.seek_to(-1, SEEK_SET),
            Err(StreamError::PositionOutOfRange)
        );
        assert_eq!(stream.seek_to(1, SEEK_SET), Ok(1));
        assert_eq!(
            stream.seek_to(i64::MAX, SEEK_CUR),
            Err(StreamError::PositionOutOfRange)
        );
        assert_eq!(
            stream.seek_to(i64::MIN, SEEK_SET),
            Err(StreamError::PositionOutOfRange)
        );
        assert_eq!(
            stream.seek_to(capacity + 1, SEEK_SET),
            Err(StreamError::CapacityExceeded)
        );
        assert_eq!(stream.position.get(), 1);
        assert_eq!(stream.stream_error(), Some(StreamError::InvalidArgument));
    }

    #[test]
    fn read_returns_available_bytes_and_advances_only_by_that_amount() {
        let stream = MemoryStream::from_bytes(b"abc").expect("small fixture fits");
        let mut output = [0u8; 5];
        let count = stream.read_at_cursor(&mut output);
        assert_eq!(count, 3);
        assert_eq!(&output[..3], b"abc");
        assert_eq!(stream.position.get(), 3);
        assert_eq!(stream.read_at_cursor(&mut output), 0);
        assert_eq!(stream.position.get(), 3);
    }

    #[test]
    fn read_after_a_sparse_seek_preserves_the_seek_position_at_eof() {
        let stream = MemoryStream::from_bytes(b"abc").expect("small fixture fits");
        assert_eq!(stream.seek_to(8, SEEK_SET), Ok(8));
        let mut output = [0u8; 1];
        assert_eq!(stream.read_at_cursor(&mut output), 0);
        assert_eq!(stream.position.get(), 8);
    }

    #[test]
    fn input_payload_over_capacity_is_rejected_before_copying() {
        let payload = vec![0u8; STATE_STREAM_CAPACITY + 1];
        assert!(matches!(
            MemoryStream::from_bytes(&payload),
            Err(StreamError::CapacityExceeded)
        ));
    }
}
