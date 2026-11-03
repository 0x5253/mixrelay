//! Admission and dispatch of bounded host PCM requests, independent of inventory control.
use crate::{
    device::Device,
    io_admission::Stage,
    registry::{IoOperation, Registry},
};
use std::sync::atomic::Ordering;

impl Registry {
    pub(crate) unsafe fn io(&self, operation: IoOperation) -> bool {
        let _audio = crate::transport::AudioScope::enter();
        let IoOperation {
            device: id,
            stream,
            render,
            samples,
            frames,
            ..
        } = operation;
        let Some(device) = self.pin(id).filter(|device| device.id == id) else {
            return false;
        };
        device
            .io_trace
            .note(if render { Stage::Write } else { Stage::Read });
        // The host can coalesce IO beyond its nominal device quantum. Bound one
        // request by the prepared retention, then process it in scratch-sized tiles.
        // A render span is bounded by the coalescing its client queue is sized for.
        let limit = if render {
            mixrelay_macos_transport::client_pcm::MAX_RENDER_SPAN_FRAMES
        } else {
            mixrelay_audio_io::retention::RING_FRAMES
        };
        if samples.is_null() || frames > limit {
            device
                .io_trace
                .reject_extent(frames, !samples.is_null(), render);
            return false;
        }
        let expected = device.id + if render { 2 } else { 1 };
        if stream != expected {
            device.io_trace.note(Stage::Stream);
            return false;
        }
        let Some(_io) = device.io_pin() else {
            device.io_trace.note(Stage::Frozen);
            return false;
        };
        // SAFETY: the IO pin fixes native geometry and the host owns the full buffer.
        if unsafe { idle_capture(&device, operation) } {
            device.io_trace.note(Stage::Inactive);
            return true;
        }
        let accepted = unsafe { process_tiles(&device, operation) };
        if !accepted {
            device.io_trace.note(Stage::Native);
        }
        accepted
    }
}

// SAFETY: caller retains one IO pin and the complete exclusive host span in the
// committed directional format. Every tile remains within that same allocation.
unsafe fn process_tiles(device: &Device, operation: IoOperation) -> bool {
    let mut transfer = crate::io_outcome::Transfer {
        direction: if operation.render {
            crate::io_outcome::Direction::ClientOutput(operation.client)
        } else {
            crate::io_outcome::Direction::Capture
        },
        frame: operation.frame,
        samples: operation.samples,
        frames: operation.frames,
    };
    if operation.frames as usize <= crate::formats::MAX_BUFFER_FRAMES {
        // SAFETY: the common callback path already fits all prepared processing storage.
        return unsafe { device.process_io(transfer) };
    }
    let channels = if operation.render {
        device.render_channels.load(Ordering::Acquire)
    } else {
        device.capture_channels.load(Ordering::Acquire)
    } as usize;
    let sample_bytes = if operation.render {
        4
    } else {
        device.capture_format.format().bytes_per_sample()
    };
    let maximum = crate::formats::BUFFER_SIZES[crate::formats::BUFFER_SIZES.len() - 1];
    let mut offset = 0;
    while offset < operation.frames {
        let frames = (operation.frames - offset).min(maximum);
        let Some(frame) = operation.frame.checked_add(u64::from(offset)) else {
            return false;
        };
        // SAFETY: validated total frame extent and the IO pin fix this tile's byte range.
        transfer.samples = unsafe {
            operation
                .samples
                .cast::<u8>()
                .add(offset as usize * channels * sample_bytes)
                .cast()
        };
        transfer.frame = frame;
        transfer.frames = frames;
        // SAFETY: tile geometry fits the prepared converter scratch and native mixer.
        if !unsafe { device.process_io(transfer) } {
            return false;
        }
        offset += frames;
    }
    true
}

// SAFETY: the caller holds an IO pin and validated the operation extent. The SDK
// owns frames * native_channels writable samples, in the pinned native format.
unsafe fn idle_capture(device: &Device, operation: IoOperation) -> bool {
    let active = if operation.render {
        &device.output_active
    } else {
        &device.input_active
    };
    let idle = !device.alive.load(Ordering::Acquire)
        || !active.load(Ordering::Acquire)
        || device.running.load(Ordering::Acquire) == 0;
    if idle && !operation.render {
        let channels = device.capture_channels.load(Ordering::Acquire) as usize;
        let bytes = operation.frames as usize
            * channels
            * device.capture_format.format().bytes_per_sample();
        // SAFETY: the IO pin fixes this validated writable native extent.
        unsafe { std::ptr::write_bytes(operation.samples.cast::<u8>(), 0, bytes) };
    }
    idle
}
