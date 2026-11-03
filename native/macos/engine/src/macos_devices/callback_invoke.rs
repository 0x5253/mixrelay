#![cfg_attr(
    any(test, feature = "test-util"),
    expect(
        clippy::expect_used,
        reason = "synthetic callback fixtures admit only bounded Float32 geometry"
    )
)]
//! One synthetic `IOProc` invocation over the production callback ABI.
use mixrelay_macos_transport::{audio_buffer::AudioBuffer, device_io::mr_device_io_process};
use std::{ffi::c_void, ptr};

/// `CoreAudio`'s `AudioTimeStamp`, the layout the native callback reads.
#[repr(C)]
#[derive(Default)]
struct Timestamp {
    sample: f64,
    host: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: [u8; 24],
    flags: u32,
    reserved: u32,
}
const _: () = assert!(size_of::<Timestamp>() == 64 && std::mem::offset_of!(Timestamp, flags) == 56);

#[repr(C)]
struct BufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

/// One callback's native timing. Host times are mach ticks.
#[derive(Clone, Copy)]
pub(crate) struct CallbackTime {
    pub(crate) now: u64,
    /// Capture time of the first input frame, or presentation time of the first output frame.
    pub(crate) host: u64,
    pub(crate) sample: f64,
    pub(crate) rate_scalar: f64,
}

/// One `IOProc` invocation of `io` over interleaved PCM: read for input, written for output.
///
/// # Safety
/// `io` is a retained native owner without an OS registration whose admitted Float32 layout has
/// `channels` channels, and the caller is its only serialized callback.
pub(crate) unsafe fn invoke(
    io: *mut c_void,
    input: bool,
    channels: usize,
    pcm: &mut [f32],
    time: CallbackTime,
) {
    let mut list = BufferList {
        count: 1,
        buffers: [AudioBuffer {
            channels: u32::try_from(channels).expect("channel count"),
            byte_size: u32::try_from(size_of_val(pcm)).expect("buffer size"),
            data: pcm.as_mut_ptr(),
        }],
    };
    let now = Timestamp {
        host: time.now,
        flags: 2,
        ..Timestamp::default()
    };
    // Sample, host and rate scalar are valid, as `CoreAudio` supplies them.
    let stream = Timestamp {
        sample: time.sample,
        host: time.host,
        rate_scalar: time.rate_scalar,
        flags: 7,
        ..Timestamp::default()
    };
    // SAFETY: the caller is this owner's only callback. The list covers exactly `pcm`,
    // and both timestamps outlive the call.
    unsafe {
        mr_device_io_process(
            0,
            ptr::from_ref(&now).cast(),
            if input {
                ptr::from_ref(&list).cast()
            } else {
                ptr::null()
            },
            if input {
                ptr::from_ref(&stream).cast()
            } else {
                ptr::null()
            },
            if input {
                ptr::null_mut()
            } else {
                ptr::from_mut(&mut list).cast()
            },
            if input {
                ptr::null()
            } else {
                ptr::from_ref(&stream).cast()
            },
            io,
        );
    }
}
