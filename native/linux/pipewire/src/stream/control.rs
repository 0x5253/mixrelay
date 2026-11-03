//! Weak native observations and pinned metadata transactions for one acquisition.
use super::{
    NativeAllocation, decode_native_format, mr_pw_stream_failed, mr_pw_stream_native_format,
    mr_pw_stream_node_id, mr_pw_stream_ready, mr_pw_stream_return_error,
    mr_pw_stream_set_description, mr_pw_stream_supported_formats,
};
use std::{
    ffi::{CStr, CString},
    sync::{Arc, Weak},
};

/// Restricted weak capability for one native acquisition; never a PCM owner.
#[derive(Clone)]
pub struct StreamProperties(Weak<NativeAllocation>);

impl StreamProperties {
    pub(super) fn new(allocation: &Arc<NativeAllocation>) -> Self {
        Self(Arc::downgrade(allocation))
    }

    /// Reports native disconnection/error of this live acquisition, excluding normal pause.
    /// Retired weak capabilities no longer refer to an owner requiring recovery.
    #[must_use]
    pub fn failed(&self) -> bool {
        let Some(allocation) = self.0.upgrade() else {
            return false;
        };
        // SAFETY: the strong allocation pins teardown during the atomic state read.
        unsafe { mr_pw_stream_failed(allocation.pointer.as_ptr()) != 0 }
    }

    /// Observes the first native buffer-return errno without resetting the acquisition.
    #[must_use]
    pub fn buffer_return_error(&self) -> Option<i32> {
        let allocation = self.0.upgrade()?;
        // SAFETY: the strong allocation pins the atomic native fault observation.
        let error = unsafe { mr_pw_stream_return_error(allocation.pointer.as_ptr()) };
        (error < 0).then_some(error)
    }

    /// Returns the registry ID of this exact, still-owned native stream.
    /// Recycled IDs must be paired with the `PipeWire` serial before use.
    #[must_use]
    pub fn node_id(&self) -> Option<u32> {
        let allocation = self.0.upgrade()?;
        // SAFETY: the strong allocation pins stream teardown; the native call
        // reads the stream's node ID under its control-loop lock.
        let id = unsafe { mr_pw_stream_node_id(allocation.pointer.as_ptr()) };
        (id != u32::MAX).then_some(id)
    }

    /// Reports the actual advertised encoding mask and coherent negotiated tuple.
    /// A retired owner returns None; paused/streaming state gates current readback.
    #[must_use]
    pub fn native_formats(&self) -> Option<(u32, Option<mixrelay_audio_io::NativeStreamFormat>)> {
        let allocation = self.0.upgrade()?;
        // SAFETY: this Arc pins the exact stream throughout immutable/atomic
        // observations. These reads neither move PCM cursors nor acquire RT locks.
        unsafe {
            let pointer = allocation.pointer.as_ptr();
            let supported = mr_pw_stream_supported_formats(pointer);
            let current = if mr_pw_stream_ready(pointer) != 0 {
                decode_native_format(mr_pw_stream_native_format(pointer))
            } else {
                None
            };
            Some((supported, current))
        }
    }

    /// Validates both strings and pins this exact live acquisition before any
    /// write. An expired capability cannot resolve a replacement by name or ID.
    /// # Errors
    /// Rejects embedded NUL bytes and already retired native allocations.
    pub fn description_change(
        &self,
        before: &str,
        after: &str,
    ) -> Result<DescriptionChange, String> {
        let before = CString::new(before).map_err(|_| "invalid previous PipeWire description")?;
        let after = CString::new(after).map_err(|_| "invalid new PipeWire description")?;
        let allocation = self
            .0
            .upgrade()
            .ok_or("PipeWire metadata owner has retired")?;
        Ok(DescriptionChange {
            allocation,
            before,
            after,
        })
    }
}

/// Control-thread-only transaction item; Drop releases its temporary native pin.
pub struct DescriptionChange {
    allocation: Arc<NativeAllocation>,
    before: CString,
    after: CString,
}

impl DescriptionChange {
    /// Applies the prepared description without changing the native PCM owner.
    /// # Errors
    /// Returns a native property write failure.
    pub fn apply(&self) -> Result<(), String> {
        self.write(&self.after)
    }

    /// Restores the previous description on this same pinned acquisition.
    /// # Errors
    /// Returns a native property write failure, including failed rollback.
    pub fn rollback(&self) -> Result<(), String> {
        self.write(&self.before)
    }

    fn write(&self, value: &CStr) -> Result<(), String> {
        // SAFETY: this transaction pins the allocation through the call. The
        // native adapter copies the C string under its control-loop lock, touches
        // no PCM cursor and cannot race destruction while this Arc is retained.
        let result = unsafe {
            mr_pw_stream_set_description(self.allocation.pointer.as_ptr(), value.as_ptr())
        };
        if result < 0 {
            return Err(format!("PipeWire description update failed: {result}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_capability_and_invalid_strings_cannot_stage_native_writes() {
        let properties = StreamProperties(Weak::new());
        assert_eq!(properties.node_id(), None);
        assert_eq!(properties.native_formats(), None);
        assert!(!properties.failed());
        assert_eq!(properties.buffer_return_error(), None);
        for (before, after, error) in [
            ("before", "after", "retired"),
            ("before\0", "after", "previous"),
            ("before", "after\0", "new"),
        ] {
            let result = properties.description_change(before, after);
            assert!(result.err().unwrap().contains(error));
        }
    }
}
