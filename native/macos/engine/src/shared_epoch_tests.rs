//! Exercise the production shared-memory epoch ABI without starting `CoreAudio`.

use mixrelay_macos_transport::shared_audio_buffer::{
    SharedAudioBuffer, mr_shared_audio_buffer_initialize, mr_shared_audio_buffer_read,
    mr_shared_audio_buffer_required_bytes, mr_shared_audio_buffer_try_begin_epoch,
    mr_shared_audio_buffer_write,
};
use std::ptr::NonNull;

struct Buffer {
    // u64 storage supplies the native layout's required eight-byte alignment.
    // This owner never resizes or exposes its backing storage after init.
    _storage: Box<[u64]>,
    pointer: NonNull<SharedAudioBuffer>,
    writer_epoch: u64,
}

impl Buffer {
    fn new() -> Self {
        // SAFETY: Pure size query for valid native geometry.
        let bytes = mr_shared_audio_buffer_required_bytes(64, 1);
        let mut storage = vec![0_u64; bytes.div_ceil(8)].into_boxed_slice();
        // SAFETY: Allocation has sufficient size/alignment and is retained until drop.
        let pointer =
            unsafe { mr_shared_audio_buffer_initialize(storage.as_mut_ptr().cast(), bytes, 64, 1) };
        Self {
            _storage: storage,
            pointer: NonNull::new(pointer).expect("valid shared buffer"),
            writer_epoch: 1,
        }
    }

    fn write(&mut self, start: u64, samples: &[f32]) {
        // SAFETY: Initialized mono buffer; the slice contains all requested frames.
        assert!(unsafe {
            mr_shared_audio_buffer_write(
                self.pointer.as_ptr(),
                self.writer_epoch,
                start,
                samples.as_ptr(),
                u32::try_from(samples.len()).unwrap(),
            )
        });
    }

    fn read(&mut self, start: u64, samples: &mut [f32]) -> u32 {
        // SAFETY: Initialized mono buffer and valid destination for the frame count.
        unsafe {
            mr_shared_audio_buffer_read(
                self.pointer.as_ptr(),
                start,
                samples.as_mut_ptr(),
                u32::try_from(samples.len()).unwrap(),
            )
        }
    }

    fn invalidate(&mut self) -> u64 {
        // SAFETY: Owner remains alive throughout the native operation.
        let epoch = unsafe { mr_shared_audio_buffer_try_begin_epoch(self.pointer.as_ptr()) };
        if epoch != 0 {
            self.writer_epoch = epoch;
        }
        epoch
    }
}

#[test]
fn bounded_invalidation_rejects_old_queued_pcm_at_reused_coordinates() {
    let mut buffer = Buffer::new();
    buffer.write(100, &[0.5; 32]);
    assert_eq!(buffer.invalidate(), 2);
    let mut output = [1.0; 32];
    assert_eq!(buffer.read(100, &mut output), 0);
    assert_eq!(output.map(f32::to_bits), [0.0_f32; 32].map(f32::to_bits));
    buffer.write(100, &[0.25; 8]);
    assert_eq!(buffer.read(100, &mut output), 8);
    assert_eq!(&output[..8], &[0.25; 8]);
    assert_eq!(&output[8..], &[0.0; 24]);
}

#[test]
fn exhausted_transport_requires_new_mapping_without_reusing_remote_generation() {
    let mut buffer = Buffer::new();
    buffer.write(0, &[0.5; 4]);
    for epoch in 2..=u64::from(u16::MAX) {
        assert_eq!(buffer.invalidate(), epoch);
    }
    assert_eq!(buffer.invalidate(), 0);
    // SAFETY: Owner is alive; control must refuse generation reuse even when its
    // local writer drained, because a remote reader may still hold old stamps.
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(buffer.pointer.as_ptr()) },
        0
    );
    let mut output = [1.0; 4];
    assert_eq!(buffer.read(0, &mut output), 0);
    assert_eq!(output.map(f32::to_bits), [0.0_f32; 4].map(f32::to_bits));
    let mut replacement = Buffer::new();
    replacement.write(0, &[0.25; 4]);
    assert_eq!(replacement.read(0, &mut output), 4);
    assert_eq!(output.map(f32::to_bits), [0.25_f32; 4].map(f32::to_bits));
}

#[test]
fn bounded_invalidation_handles_missing_transport() {
    // SAFETY: The ABI explicitly supports a null/missing transport.
    assert_eq!(
        unsafe { mr_shared_audio_buffer_try_begin_epoch(std::ptr::null_mut()) },
        0
    );
}
