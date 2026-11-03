use super::*;

#[repr(C)]
struct List<const N: usize> {
    count: u32,
    buffers: [AudioBuffer; N],
}

fn descriptor(pcm: &mut [f32], channels: u32) -> AudioBuffer {
    AudioBuffer {
        channels,
        byte_size: u32::try_from(size_of_val(pcm)).unwrap(),
        data: pcm.as_mut_ptr(),
    }
}

fn binding(offset: usize, channels: usize) -> Binding {
    Binding {
        id: "test".into(),
        input: true,
        offset: Some(offset),
        channels,
    }
}

fn inspect<const N: usize, R>(
    input: &List<N>,
    frames: u32,
    operation: impl for<'a> FnOnce(Buffers<'a>) -> R,
) -> Option<R> {
    // SAFETY: each fixture owns a sized list and retains its referenced PCM across
    // inspect. Invalid geometry fixtures are rejected before any PCM is borrowed.
    unsafe {
        Buffers::with_native(
            ptr::from_ref(input).cast(),
            ptr::null_mut(),
            frames,
            operation,
        )
    }
}

#[test]
fn empty_and_missing_native_directions_are_valid() {
    let empty = List::<0> {
        count: 0,
        buffers: [],
    };
    inspect(&empty, 32, |view| assert!(!view.covers(&binding(0, 1)))).unwrap();
    // SAFETY: null directions own no metadata or PCM.
    unsafe {
        Buffers::with_native(ptr::null(), ptr::null_mut(), 32, |view| {
            let mut offline = binding(0, 1);
            offline.offset = None;
            assert!(view.covers(&offline));
        })
    }
    .unwrap();
}

#[test]
fn malformed_geometry_is_rejected_before_pcm_access() {
    let mut pcm = [0.0_f32; 64];
    for channels in [0, 1, 3, 2049, u32::MAX] {
        let list = List {
            count: 1,
            buffers: [descriptor(&mut pcm, channels)],
        };
        assert!(inspect(&list, 32, |_| ()).is_none());
    }
    let mut list = List {
        count: 1,
        buffers: [descriptor(&mut pcm, 2)],
    };
    for frames in [0, 31, 33, 4097, u32::MAX] {
        assert!(inspect(&list, frames, |_| ()).is_none());
    }
    list.count = 2049;
    assert!(inspect(&list, 32, |_| ()).is_none());
    list.count = 1;
    list.buffers[0].data = pcm.as_mut_ptr().cast::<u8>().wrapping_add(1).cast();
    assert!(inspect(&list, 32, |_| ()).is_none());
    list.buffers[0].data = ptr::without_provenance_mut(usize::MAX - 3);
    assert!(inspect(&list, 32, |_| ()).is_none());
    // SAFETY: alignment is rejected without reading this malformed header address.
    assert!(
        unsafe {
            Buffers::with_native(
                ptr::from_ref(&list).cast::<u8>().wrapping_add(1).cast(),
                ptr::null_mut(),
                32,
                |_| (),
            )
        }
        .is_none()
    );
}

#[test]
fn aggregate_width_2048_is_supported_but_overflow_is_rejected() {
    let mut pcm = vec![0.25_f32; 2048];
    let list = List {
        count: 1,
        buffers: [descriptor(&mut pcm, 2048)],
    };
    let map = binding(2046, 2);
    let mut destination = [0.0; 2];
    inspect(&list, 1, |view| {
        assert!(view.covers(&map));
        view.transfer(&map, 0, &mut destination);
    })
    .unwrap();
    assert_eq!(destination.map(f32::to_bits), [0.25_f32.to_bits(); 2]);
    let too_wide = List {
        count: 2,
        buffers: [
            list.buffers[0],
            AudioBuffer {
                channels: 1,
                byte_size: 0,
                data: ptr::null_mut(),
            },
        ],
    };
    assert!(inspect(&too_wide, 1, |_| ()).is_none());
}

#[test]
fn invalid_safe_transfer_requests_do_not_touch_pcm() {
    let mut pcm = [0.25_f32; 64];
    let list = List {
        count: 1,
        buffers: [descriptor(&mut pcm, 2)],
    };
    let mut output = List {
        count: 1,
        buffers: list.buffers,
    };
    let mut destination = [9.0_f32; 64];
    // SAFETY: input/output share this fixture's allocation deliberately; methods
    // borrow one native extent at a time and the DSP scratch remains disjoint.
    unsafe {
        Buffers::with_native(
            ptr::from_ref(&list).cast(),
            ptr::from_mut(&mut output).cast(),
            32,
            |mut view| {
                for map in [binding(0, 0), binding(usize::MAX, 2), binding(2048, 1)] {
                    assert!(!view.covers(&map));
                    view.transfer(&map, 0, &mut destination);
                    view.write(&map, 0, &destination);
                }
                for offset in [1, 32, usize::MAX] {
                    view.transfer(&binding(0, 2), offset, &mut destination);
                    view.write(&binding(0, 2), offset, &destination);
                }
                view.transfer(&binding(0, 2), 0, &mut destination[..3]);
                view.write(&binding(0, 2), 0, &destination[..3]);
                view.transfer(&binding(1, 1), 0, &mut []);
                view.write(&binding(1, 1), 0, &[]);
            },
        )
    }
    .unwrap();
    assert_eq!(destination.map(f32::to_bits), [9.0_f32.to_bits(); 64]);
    assert_eq!(pcm.map(f32::to_bits), [0.25_f32.to_bits(); 64]);
}

#[test]
fn mixed_packed_and_planar_channels_preserve_offsets_and_accumulate() {
    let mut packed = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let mut planar = [7.0_f32, 8.0, 9.0];
    let mut out_packed = [0.0_f32; 6];
    let mut out_planar = [0.0_f32; 3];
    let input = List {
        count: 2,
        buffers: [descriptor(&mut packed, 2), descriptor(&mut planar, 1)],
    };
    let mut output = List {
        count: 2,
        buffers: [
            descriptor(&mut out_packed, 2),
            descriptor(&mut out_planar, 1),
        ],
    };
    let mut destination = [0.0_f32; 4];
    let map = binding(1, 2);
    // SAFETY: sized input/output metadata and their four PCM allocations are
    // retained; destination is separately owned interleaved DSP scratch.
    unsafe {
        Buffers::with_native(
            ptr::from_ref(&input).cast(),
            ptr::from_mut(&mut output).cast(),
            3,
            |mut view| {
                view.transfer(&map, 1, &mut destination);
                view.write(&map, 1, &destination);
                view.write(&map, 1, &destination);
            },
        )
    }
    .unwrap();
    assert_eq!(
        destination.map(f32::to_bits),
        [4.0_f32, 8.0, 6.0, 9.0].map(f32::to_bits)
    );
    assert_eq!(
        out_packed.map(f32::to_bits),
        [0.0_f32, 0.0, 0.0, 8.0, 0.0, 12.0].map(f32::to_bits)
    );
    assert_eq!(
        out_planar.map(f32::to_bits),
        [0.0_f32, 16.0, 18.0].map(f32::to_bits)
    );
}

fn layout(pcm: &mut [f32], width: usize, planar: bool) -> List<64> {
    let mut list = List {
        count: u32::try_from(if planar { width } else { 1 }).unwrap(),
        buffers: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: ptr::null_mut(),
        }; 64],
    };
    let channels = if planar { 1 } else { width };
    for (buffer, samples) in list.buffers.iter_mut().zip(pcm.chunks_mut(7 * channels)) {
        *buffer = descriptor(samples, u32::try_from(channels).unwrap());
    }
    list
}

#[test]
fn nonuniform_pcm_preserves_channel_and_frame_positions_in_every_layout() {
    for width in [2, 8, 64] {
        for planar in [false, true] {
            for channel_offset in [0, 1] {
                check_nonuniform_pcm(width, planar, channel_offset);
            }
        }
    }
}

fn check_nonuniform_pcm(width: usize, planar: bool, channel_offset: usize) {
    let mut input: Vec<_> = (0..width * 7)
        .map(|index| f32::from(u16::try_from(index + 1).unwrap()))
        .collect();
    let mut output = vec![0.0_f32; input.len()];
    let native_index = |frame: usize, channel: usize| {
        if planar {
            channel * 7 + frame
        } else {
            frame * width + channel
        }
    };
    let channels = width - channel_offset;
    let mut expected = Vec::new();
    let mut expected_output = vec![0.0_f32; output.len()];
    for frame in 2..5 {
        for channel in channel_offset..width {
            let index = native_index(frame, channel);
            expected.push(input[index]);
            expected_output[index] = input[index] * 2.0;
        }
    }
    let mut destination = vec![0.0_f32; expected.len()];
    let input_list = layout(&mut input, width, planar);
    let mut output_list = layout(&mut output, width, planar);
    let map = binding(channel_offset, channels);
    // SAFETY: sized metadata lists retain separate seven-frame native allocations
    // throughout the callback; graph destination is independently allocated.
    unsafe {
        Buffers::with_native(
            ptr::from_ref(&input_list).cast(),
            ptr::from_mut(&mut output_list).cast(),
            7,
            |mut view| {
                view.transfer(&map, 2, &mut destination);
                view.write(&map, 2, &destination);
                view.write(&map, 2, &destination);
            },
        )
    }
    .unwrap();
    assert_eq!(destination, expected);
    assert_eq!(output, expected_output);
}

#[test]
fn overlapping_native_pcm_is_borrowed_sequentially_without_allocation() {
    let mut pcm = [0.25_f32; 64];
    let buffer = descriptor(&mut pcm, 2);
    let input = List {
        count: 1,
        buffers: [buffer],
    };
    let mut output = List {
        count: 2,
        buffers: [buffer, buffer],
    };
    let map = binding(0, 4);
    let input_map = binding(0, 2);
    let mut destination = [0.0_f32; 128];
    let allocations = allocation_counter::measure(|| {
        // SAFETY: deliberately aliased native descriptors retain the same PCM,
        // accessed exclusively through sequential view operations. Graph scratch
        // is separate and the measured operation does not allocate or retain views.
        unsafe {
            Buffers::with_native(
                ptr::from_ref(&input).cast(),
                ptr::from_mut(&mut output).cast(),
                32,
                |mut view| {
                    view.transfer(&input_map, 0, &mut destination[..64]);
                    view.write(&map, 0, &destination);
                    view.transfer(&input_map, 0, &mut destination[..64]);
                },
            )
        }
        .unwrap();
    });
    assert_eq!(allocations.count_total, 0);
    let expected = std::array::from_fn::<_, 64, _>(|index| {
        if index < 32 {
            0.75_f32.to_bits()
        } else {
            0.25_f32.to_bits()
        }
    });
    assert_eq!(pcm.map(f32::to_bits), expected);
    assert_eq!(&destination.map(f32::to_bits)[..64], &expected);
}
