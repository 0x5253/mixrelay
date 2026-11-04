use super::*;
use crate::{
    aggregate_group::NativeBinding,
    audio_buffer::AudioBuffer,
    callback_clock::{CallbackClockSnapshot, mr_callback_clock_snapshot},
    device_io::lifecycle::{Backend, Created},
};
use std::{
    cell::Cell,
    ffi::c_void,
    ptr::{self, NonNull},
    sync::atomic::Ordering::{Acquire, Release},
};

mod io_proc;
mod layouts;
mod native_pcm;
mod pcm_hook;
mod properties;
mod queue_observation;
mod registration;
mod timestamps;

#[repr(C)]
pub(super) struct Buffers {
    pub(super) count: u32,
    pub(super) buffers: [AudioBuffer; 64],
}

pub(super) fn buffers(samples: &mut [f32], channels: u32, frames: u32, planar: bool) -> Buffers {
    let mut result = Buffers {
        count: if planar { channels } else { 1 },
        buffers: std::array::from_fn(|_| AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: ptr::null_mut(),
        }),
    };
    for index in 0..result.count as usize {
        let width = if planar { 1 } else { channels };
        result.buffers[index] = AudioBuffer {
            channels: width,
            byte_size: frames * width * 4,
            // SAFETY: test allocation contains count*frames*width floats.
            data: unsafe {
                samples
                    .as_mut_ptr()
                    .add(if planar { index * frames as usize } else { 0 })
            },
        };
    }
    result
}

pub(super) fn owner(input: bool, channels: u32) -> DeviceIo {
    let io = DeviceIo::for_test(c"unit", input, channels, 48000).expect("test owner allocation");
    // SAFETY: unpublished fixture exclusively owns callback state before exercising IOProc.
    assert!(unsafe {
        super::test_observation::admit_float_callback(ptr::from_ref(&io).cast_mut())
    });
    io
}

pub(super) fn process(io: &DeviceIo, buffers: &mut Buffers) {
    // Fixtures supply the same control-admitted metadata that real prepare queries from ASBD.
    if io.progress.ticks.load(Acquire) == 0 && !io.progress.invalid.load(Acquire) {
        let mut formats = crate::device_buffers::NativeFormats::default();
        for buffer in &buffers.buffers[..buffers.count as usize] {
            assert!(formats.append(
                buffer.channels,
                false,
                mixrelay_pcm_ring::SampleFormat::Float32
            ));
        }
        // SAFETY: unit fixture has no registered callback or concurrent control owner.
        unsafe {
            (*io.callback.get()).formats = formats;
        }
    }
    dispatch_callback(io, buffers);
}

pub(super) fn dispatch_callback(io: &DeviceIo, buffers: &mut Buffers) {
    let native = ptr::from_mut(buffers).cast();
    // SAFETY: tests serialize each role and supply complete metadata/PCM allocations.
    unsafe {
        mr_device_io_process(
            0,
            ptr::null(),
            if io.selection.input {
                native
            } else {
                ptr::null()
            },
            ptr::null(),
            if io.selection.input {
                ptr::null_mut()
            } else {
                native
            },
            ptr::null(),
            ptr::from_ref(io).cast_mut().cast(),
        );
    }
}

pub(super) fn sample(value: impl TryInto<u16>) -> f32 {
    f32::from(value.try_into().ok().expect("test sample values fit u16"))
}

pub(super) fn assert_pcm(left: &[f32], right: &[f32]) {
    assert_eq!(left.len(), right.len());
    for (left, right) in left.iter().zip(right) {
        assert_eq!(left.to_bits(), right.to_bits());
    }
}

pub(super) fn samples(channels: u32, frames: u32) -> Vec<f32> {
    (0..channels * frames)
        .map(|index| sample(index % channels + 1) / 128.0)
        .collect()
}

#[derive(Default)]
pub(super) struct RegistrationStub {
    pub(super) create_status: i32,
    pub(super) start_status: i32,
    pub(super) destroy_status: i32,
    pub(super) prepares: usize,
    pub(super) starts: usize,
    pub(super) stops: usize,
    pub(super) destroys: usize,
}

pub(super) struct StubBackend<'a> {
    pub(super) io: &'a DeviceIo,
    pub(super) native: &'a mut RegistrationStub,
}

impl Backend for StubBackend<'_> {
    fn prepare(&mut self, _: u32) -> Result<u32, i32> {
        self.native.prepares += 1;
        Ok(123)
    }
    fn create(&mut self, _: u32) -> Created {
        Created {
            procedure: Some(NonNull::dangling()),
            status: self.native.create_status,
        }
    }
    fn start(&mut self, _: u32, _: NonNull<c_void>) -> i32 {
        self.native.starts += 1;
        self.native.start_status
    }
    fn silence(&mut self) {
        self.io
            .progress
            .invalid
            .store(true, std::sync::atomic::Ordering::Release);
    }
    fn stop(&mut self, _: u32, _: NonNull<c_void>) -> i32 {
        self.native.stops += 1;
        23
    }
    fn destroy(&mut self, _: u32, _: NonNull<c_void>) -> i32 {
        self.native.destroys += 1;
        self.native.destroy_status
    }
}

pub(super) fn register(io: &DeviceIo, native: &mut RegistrationStub) -> i32 {
    // SAFETY: unit fixture owns Control exclusively; stub callback does not access it.
    unsafe {
        (*io.control.get())
            .lifecycle
            .connect(0, &mut StubBackend { io, native })
    }
}

pub(super) fn retire(io: *mut DeviceIo, native: &mut RegistrationStub) -> bool {
    // SAFETY: exact production allocation release path with a synchronous mock
    // native acknowledgement. No worker/graph survives this test-owned allocation.
    unsafe {
        ffi::destroy_with(io, |io| {
            (*io.control.get())
                .lifecycle
                .disconnect(&mut StubBackend { io, native })
        })
    }
}

pub(super) struct PropertiesStub {
    pub(super) rate: f64,
    pub(super) default: u32,
    pub(super) transport: Option<u32>,
    pub(super) stream_size: u32,
    pub(super) default_queries: Cell<u32>,
    pub(super) pcm: mixrelay_pcm_ring::SampleFormat,
}

impl Default for PropertiesStub {
    fn default() -> Self {
        Self {
            rate: 48000.0,
            default: 234,
            transport: Some(u32::from_be_bytes(*b"usb ")),
            stream_size: 4,
            default_queries: Cell::new(0),
            pcm: mixrelay_pcm_ring::SampleFormat::Float32,
        }
    }
}

fn scalar_copy<T: Copy + Default, U: Copy>(value: U) -> Option<T> {
    if size_of::<T>() != size_of::<U>() {
        return None;
    }
    let mut result = T::default();
    // SAFETY: property test supplies the exact private query type and checked ABI size.
    unsafe {
        ptr::copy_nonoverlapping(
            ptr::from_ref(&value).cast::<u8>(),
            ptr::from_mut(&mut result).cast(),
            size_of::<T>(),
        );
    }
    Some(result)
}

impl native::Properties for PropertiesStub {
    fn read<T: Copy + Default>(&self, _: u32, address: native::Address) -> Option<T> {
        match address.selector.to_be_bytes() {
            [b'd', b'I', b'n', b' '] | [b'd', b'O', b'u', b't'] => {
                self.default_queries.set(self.default_queries.get() + 1);
                scalar_copy(self.default)
            }
            [b't', b'r', b'a', b'n'] => self.transport.and_then(scalar_copy),
            [b'l', b'i', b'v', b'n'] => scalar_copy(1_u32),
            [b's', b'f', b'm', b't'] => scalar_copy(native::Description {
                rate: self.rate,
                format: u32::from_be_bytes(*b"lpcm"),
                flags: if self.pcm == mixrelay_pcm_ring::SampleFormat::Float32 {
                    1
                } else {
                    12
                },
                bits: u32::try_from(self.pcm.bytes_per_sample() * 8).unwrap(),
                channels: 2,
                frame_bytes: u32::try_from(self.pcm.bytes_per_sample() * 2).unwrap(),
                packet_bytes: u32::try_from(self.pcm.bytes_per_sample() * 2).unwrap(),
                packet_frames: 1,
                ..native::Description::default()
            }),
            _ => None,
        }
    }
    fn streams(&self, _: u32, _: u32, _: u32) -> Option<native::StreamIds> {
        native::valid_stream_extent(self.stream_size, 4).then_some(native::StreamIds {
            values: [234; 2048],
            count: self.stream_size as usize / 4,
        })
    }
}

pub(super) unsafe extern "C" fn render_hook(
    context: *mut c_void,
    _: *const c_void,
    output: *mut c_void,
    frames: u32,
    _: *const crate::device_io::IoTiming,
) -> bool {
    // SAFETY: hook test retains the Cell and full two-channel packed output.
    let calls = unsafe { &*context.cast::<Cell<u32>>() };
    calls.set(calls.get() + 1);
    // SAFETY: this fixture supplies a complete 64-entry metadata container.
    let list = unsafe { &*output.cast::<Buffers>() };
    // SAFETY: this callback exclusively owns frames*2 initialized output floats.
    unsafe { std::slice::from_raw_parts_mut(list.buffers[0].data, frames as usize * 2) }.fill(0.5);
    true
}
