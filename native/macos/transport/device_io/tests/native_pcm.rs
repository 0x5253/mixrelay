use mixrelay_pcm_ring::SampleFormat;

use super::*;

fn integer_buffers(storage: &mut [u32], pcm: SampleFormat, planar: bool) -> Buffers {
    let mut list = Buffers {
        count: if planar { 2 } else { 1 },
        buffers: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: ptr::null_mut(),
        }; 64],
    };
    for index in 0..list.count as usize {
        let channels = if planar { 1 } else { 2 };
        let bytes = 64 * channels * pcm.bytes_per_sample();
        list.buffers[index] = AudioBuffer {
            channels: u32::try_from(channels).unwrap(),
            byte_size: u32::try_from(bytes).unwrap(),
            data: storage
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(index * bytes)
                .cast(),
        };
    }
    list
}

fn admit(io: &DeviceIo, pcm: SampleFormat, planar: bool) {
    let mut formats = NativeFormats::default();
    assert!(formats.append(2, planar, pcm));
    // SAFETY: unpublished synchronous fixture owns callback state; no registration survives.
    unsafe {
        (*io.callback.get()).formats = formats;
    }
}

#[test]
fn integer_native_callback_uses_f32_queues_and_never_calls_direct_float_hook() {
    for pcm in [
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        for planar in [false, true] {
            let input = owner(true, 2);
            let output = owner(false, 2);
            admit(&input, pcm, planar);
            admit(&output, pcm, planar);
            let source: Vec<f32> = (0..128)
                .map(|index| if index % 2 == 0 { 0.25 } else { -0.5 })
                .collect();
            let native: Vec<f32> = if planar {
                [vec![0.25; 64], vec![-0.5; 64]].concat()
            } else {
                source.clone()
            };
            let byte_count = source.len() * pcm.bytes_per_sample();
            let mut storage = vec![0_u32; byte_count.div_ceil(4)];
            // SAFETY: test owns this complete initialized native byte extent.
            assert!(pcm.encode(&native, unsafe {
                std::slice::from_raw_parts_mut(storage.as_mut_ptr().cast(), byte_count)
            }));
            let mut list = integer_buffers(&mut storage, pcm, planar);
            let mut copy = [0.0; 128];
            let calls = Cell::new(0_u32);
            // SAFETY: hook and its counter remain retained until synchronous IOProc returns.
            unsafe {
                mr_device_io_render_hook(
                    ptr::from_ref(&output).cast_mut(),
                    ptr::from_ref(&calls).cast_mut().cast(),
                    Some(render_hook),
                );
            }
            let allocation = allocation_counter::measure(|| {
                dispatch_callback(&input, &mut list);
                // SAFETY: each synchronous worker owns its opposite native ring cursor.
                unsafe {
                    assert_eq!(queues::bridge_read(&input, copy.as_mut_ptr(), 64), 64);
                    assert_eq!(queues::bridge_write(&output, copy.as_ptr(), 64), 64);
                }
                dispatch_callback(&output, &mut list);
            });
            assert_eq!(allocation.count_total, 0);
            assert_pcm(&copy, &source);
            assert_eq!(calls.get(), 0);
            let mut decoded = [0.0; 128];
            // SAFETY: output callback has ended, allowing immutable native byte readback.
            assert!(pcm.decode(
                unsafe { std::slice::from_raw_parts(storage.as_ptr().cast(), byte_count) },
                &mut decoded
            ));
            assert_pcm(&decoded, &native);
            assert_eq!(output.native_ring.stats().queued, 0);
            assert!(!output.progress.invalid.load(Acquire));
        }
    }
}

#[test]
fn actual_integer_property_admission_excludes_float_direct_binding() {
    let io = owner(false, 2);
    let binding = NativeBinding {
        device: 123,
        offset: 0,
        total: 0,
    };
    for pcm in [
        SampleFormat::Float32,
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        let format = native::admitted_format(
            &io.selection,
            binding,
            &PropertiesStub {
                pcm,
                ..PropertiesStub::default()
            },
        )
        .unwrap();
        assert_eq!(format.rate, 48_000);
        assert_eq!(format.layout.all_float(), pcm == SampleFormat::Float32);
    }
}
