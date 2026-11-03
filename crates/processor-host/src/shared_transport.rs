use std::{
    mem::{align_of, size_of},
    sync::Arc,
};

use crate::{AudioLayout, SHARED_MAGIC, SHARED_VERSION};

mod audio;
mod file;
mod region;
#[cfg(test)]
pub(super) use file::create;
pub(super) use file::{create_owned, worker};
use region::{TransportLayoutError, TransportMemoryError, TransportRegion, WorkerWake};
mod control;
mod header;
#[cfg(test)]
pub(crate) mod memory;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod variable_pcm_tests;

#[cfg(test)]
mod response_measurement;

#[cfg(test)]
mod chunk_tests;

pub(super) use audio::{ParentAudio, WorkerAudio};
pub(super) use control::{ControlMailbox, WorkerControl};
pub(super) use header::SharedHeader;

/// One bounded state response chunk shared by the parent and worker mappings.
pub(super) const CHUNK_CAPACITY: usize = 256 * 1024;

/// Owns one address-space view without ever borrowing the entire shared mapping.
/// Header atomics, the control chunk and the two PCM regions have independent ownership protocols.
struct Mapping {
    raw: Arc<dyn TransportRegion>,
    layout: AudioLayout,
    wake: Arc<dyn WorkerWake>,
}

impl Mapping {
    fn admit(
        raw: Arc<dyn TransportRegion>,
        wake: Arc<dyn WorkerWake>,
    ) -> Result<Self, TransportMemoryError> {
        if raw.byte_length() < size_of::<SharedHeader>()
            || raw.as_ptr().align_offset(align_of::<SharedHeader>()) != 0
        {
            return Err(TransportMemoryError::TruncatedOrMisaligned);
        }
        // SAFETY: the private, fixed-size transport is initialized before either peer opens it.
        // Only this header is borrowed; its live mutable fields are atomics. PCM remains raw.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "The preceding admission check validates SharedHeader alignment."
        )]
        let header = unsafe { &*raw.as_ptr().cast::<SharedHeader>() };
        if header.magic != SHARED_MAGIC || header.version != SHARED_VERSION {
            return Err(TransportMemoryError::IncompatibleHeader);
        }
        if crate::instance_id::InstanceId::new(header.instance_id).is_none() {
            return Err(TransportMemoryError::ZeroInstance);
        }
        let layout = AudioLayout {
            inputs: header.input_channels as usize,
            outputs: header.output_channels as usize,
            block_frames: header.block_frames as usize,
        };
        if raw.byte_length() != shared_length(layout)? {
            return Err(TransportMemoryError::InvalidSize);
        }
        Ok(Self { raw, layout, wake })
    }

    fn header(&self) -> &SharedHeader {
        // SAFETY: admission validates header size/alignment. The owner retains the mapping; immutable
        // metadata never changes after creation, and every writable header field is atomic.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Mapping admission validates SharedHeader alignment once before publication."
        )]
        unsafe {
            &*self.raw.as_ptr().cast::<SharedHeader>()
        }
    }

    fn input_samples(&self) -> usize {
        self.layout.inputs * self.layout.block_frames
    }

    fn input_sample_count(&self, frames: usize) -> Option<usize> {
        (frames <= self.layout.block_frames)
            .then_some(())
            .and_then(|()| self.layout.inputs.checked_mul(frames))
    }

    fn output_samples(&self) -> usize {
        self.layout.outputs * self.layout.block_frames
    }

    fn output_sample_count(&self, frames: usize) -> Option<usize> {
        (frames <= self.layout.block_frames)
            .then_some(())
            .and_then(|()| self.layout.outputs.checked_mul(frames))
    }

    fn output_offset(&self) -> usize {
        input_offset() + self.input_samples() * size_of::<f32>()
    }

    fn write_state_chunk(&self, bytes: &[u8]) {
        debug_assert!(bytes.len() <= CHUNK_CAPACITY);
        // SAFETY: Mapping admission proves that the state chunk lies fully within this mapping.
        // The worker calls this only while state_written == state_consumed, so the parent has
        // finished reading the previous chunk. The release publication follows the copy.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.raw.as_mut_ptr().add(size_of::<SharedHeader>()),
                bytes.len(),
            );
        }
    }

    fn append_state_chunk(&self, bytes: &mut Vec<u8>, length: usize) {
        debug_assert!(length <= CHUNK_CAPACITY);
        // SAFETY: Mapping admission proves that the state chunk lies fully within this mapping.
        // The parent calls this only after acquiring state_written and before acknowledging the
        // same cumulative count through state_consumed, so the worker cannot overwrite it.
        let source = unsafe {
            std::slice::from_raw_parts(self.raw.as_ptr().add(size_of::<SharedHeader>()), length)
        };
        // The caller fallibly reserved this space before borrowing the published chunk.
        bytes.extend_from_slice(source);
    }
}

/// # Safety
/// The caller exclusively owns an unpublished region and initializes its header only once.
unsafe fn initialize_private(
    raw: &dyn TransportRegion,
    header: SharedHeader,
) -> Result<(), TransportMemoryError> {
    if raw.byte_length() < size_of::<SharedHeader>()
        || raw.as_ptr().align_offset(align_of::<SharedHeader>()) != 0
    {
        return Err(TransportMemoryError::TruncatedOrMisaligned);
    }
    // SAFETY: the caller owns this unpublished region; admission above proves size/alignment.
    #[expect(
        clippy::cast_ptr_alignment,
        reason = "Initialization validates SharedHeader alignment before writing."
    )]
    unsafe {
        raw.as_mut_ptr().cast::<SharedHeader>().write(header);
    }
    Ok(())
}

fn shared_length(layout: AudioLayout) -> Result<usize, TransportLayoutError> {
    let samples = layout
        .inputs
        .checked_add(layout.outputs)
        .and_then(|channels| channels.checked_mul(layout.block_frames))
        .ok_or(TransportLayoutError::SampleCount)?;
    input_offset()
        .checked_add(
            samples
                .checked_mul(size_of::<f32>())
                .ok_or(TransportLayoutError::ByteCount)?,
        )
        .filter(|length| isize::try_from(*length).is_ok())
        .ok_or(TransportLayoutError::MappingSize)
}

const fn input_offset() -> usize {
    size_of::<SharedHeader>() + CHUNK_CAPACITY
}
