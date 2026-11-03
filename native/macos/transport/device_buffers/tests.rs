use super::*;

mod native_pcm;

#[repr(C)]
struct Fixture {
    count: u32,
    buffers: [AudioBuffer; 64],
}

#[test]
fn native_abi_and_all_channel_widths_preserve_pcm_bits_without_allocating() {
    assert_eq!(size_of::<BufferSelection>(), 16);
    assert_eq!(std::mem::offset_of!(Fixture, buffers), 8);
    for width in 1_u32..=64 {
        let channels = width as usize;
        for planar in [false, true] {
            let frames = 7;
            let mut native = vec![0.0_f32; frames * channels];
            let mut scratch = vec![0.0_f32; native.len()];
            let mut fixture = Fixture {
                count: if planar { width } else { 1 },
                buffers: [AudioBuffer {
                    channels: 0,
                    byte_size: 0,
                    data: std::ptr::null_mut(),
                }; 64],
            };
            for b in 0..fixture.count as usize {
                fixture.buffers[b] = AudioBuffer {
                    channels: if planar { 1 } else { width },
                    byte_size: u32::try_from(frames * if planar { 1 } else { channels } * 4)
                        .unwrap(),
                    data: native
                        .as_mut_ptr()
                        .wrapping_add(if planar { b * frames } else { 0 }),
                };
            }
            for f in 0..frames {
                for c in 0..channels {
                    native[if planar {
                        c * frames + f
                    } else {
                        f * channels + c
                    }] = f32::from_bits(0x7fc0_0000 + u32::try_from(f * channels + c).unwrap());
                }
            }
            let selection = BufferSelection {
                channels: width,
                offset: 0,
                total: 0,
                limit: 64,
            };
            let list = (&raw const fixture).cast();
            let allocated = allocation_counter::measure(|| {
                // SAFETY: complete native list, aligned disjoint source/destination extents.
                unsafe {
                    assert_eq!(
                        mr_device_buffers_frames(list, selection),
                        u32::try_from(frames).unwrap()
                    );
                    assert!(mr_device_buffers_transfer(
                        list,
                        selection,
                        scratch.as_mut_ptr(),
                        true
                    ));
                }
            });
            assert_eq!(allocated.count_total, 0);
            for (i, sample) in scratch.iter().enumerate() {
                assert_eq!(sample.to_bits(), 0x7fc0_0000 + u32::try_from(i).unwrap());
            }
            native.fill(0.0);
            // SAFETY: same owned spans, now scratch is input and native is exclusive output.
            assert!(unsafe {
                mr_device_buffers_transfer(list, selection, scratch.as_mut_ptr(), false)
            });
            for f in 0..frames {
                for c in 0..channels {
                    assert_eq!(
                        native[if planar {
                            c * frames + f
                        } else {
                            f * channels + c
                        }]
                        .to_bits(),
                        scratch[f * channels + c].to_bits()
                    );
                }
            }
        }
    }
}

#[test]
fn disabled_unselected_stream_keeps_its_offset_and_mixed_projection_preserves_neighbors() {
    let mut mono = [11.0, 12.0];
    let mut triple = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let buffers = [
        AudioBuffer {
            channels: 2,
            byte_size: 0,
            data: std::ptr::null_mut(),
        },
        AudioBuffer {
            channels: 1,
            byte_size: 8,
            data: mono.as_mut_ptr(),
        },
        AudioBuffer {
            channels: 3,
            byte_size: 24,
            data: triple.as_mut_ptr(),
        },
    ];
    let selection = BufferSelection {
        channels: 2,
        offset: 4,
        total: 6,
        limit: 2048,
    };
    let layout = Layout::validate(&buffers, selection).unwrap();
    let mut scratch = [0.0; 4];
    // SAFETY: two selected channels in a live triple stream; disjoint four-float scratch.
    unsafe {
        layout.capture(scratch.as_mut_ptr());
    }
    assert_eq!(
        scratch.map(f32::to_bits),
        [2.0, 3.0, 5.0, 6.0].map(f32::to_bits)
    );
    scratch.fill(9.0);
    // SAFETY: this test owns both writable native streams and disjoint scratch.
    unsafe {
        layout.render(scratch.as_ptr());
    }
    assert_eq!(mono.map(f32::to_bits), [11.0, 12.0].map(f32::to_bits));
    assert_eq!(
        triple.map(f32::to_bits),
        [1.0, 9.0, 9.0, 4.0, 9.0, 9.0].map(f32::to_bits)
    );
    assert!(
        Layout::validate(
            &buffers,
            BufferSelection {
                offset: 1,
                ..selection
            }
        )
        .is_none()
    );
}

#[test]
fn malformed_geometry_is_rejected_before_pcm_access() {
    let mut samples = [0.0_f32; 16];
    let valid = AudioBuffer {
        channels: 2,
        byte_size: 64,
        data: samples.as_mut_ptr(),
    };
    let selection = BufferSelection {
        channels: 2,
        offset: 0,
        total: 0,
        limit: 64,
    };
    for buffer in [
        AudioBuffer {
            channels: 0,
            ..valid
        },
        AudioBuffer {
            channels: 65,
            ..valid
        },
        AudioBuffer {
            byte_size: 63,
            ..valid
        },
        AudioBuffer {
            byte_size: 0,
            ..valid
        },
        AudioBuffer {
            byte_size: 4097 * 8,
            ..valid
        },
        AudioBuffer {
            data: valid.data.wrapping_byte_add(1),
            ..valid
        },
    ] {
        assert!(Layout::validate(&[buffer], selection).is_none());
    }
    for invalid in [
        BufferSelection {
            channels: 0,
            ..selection
        },
        BufferSelection {
            offset: u32::MAX,
            ..selection
        },
        BufferSelection {
            offset: 1,
            ..selection
        },
        BufferSelection {
            total: 3,
            ..selection
        },
        BufferSelection {
            limit: 65,
            ..selection
        },
    ] {
        assert!(Layout::validate(&[valid], invalid).is_none());
    }
    assert!(
        Layout::validate(
            &[
                valid,
                AudioBuffer {
                    byte_size: 32,
                    ..valid
                }
            ],
            selection
        )
        .is_none()
    );
}

fn sample(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap())
}

#[test]
fn aggregate_selection_crosses_streams_and_reaches_channels_above_64() {
    let mut first = std::array::from_fn::<_, 128, _>(|i| sample(i / 64 * 128 + i % 64));
    let mut second = std::array::from_fn::<_, 128, _>(|i| sample(i / 64 * 128 + i % 64 + 64));
    let before_first = first;
    let before_second = second;
    let buffers = [
        AudioBuffer {
            channels: 64,
            byte_size: 512,
            data: first.as_mut_ptr(),
        },
        AudioBuffer {
            channels: 64,
            byte_size: 512,
            data: second.as_mut_ptr(),
        },
    ];
    for offset in [63, 100] {
        let layout = Layout::validate(
            &buffers,
            BufferSelection {
                channels: 2,
                offset,
                total: 128,
                limit: 2048,
            },
        )
        .unwrap();
        let mut scratch = [0.0; 4];
        // SAFETY: complete two-stream aggregate fixture and disjoint stereo scratch.
        unsafe {
            layout.capture(scratch.as_mut_ptr());
        }
        assert_eq!(
            scratch.map(f32::to_bits),
            [
                sample(offset as usize),
                sample(offset as usize + 1),
                sample(offset as usize + 128),
                sample(offset as usize + 129)
            ]
            .map(f32::to_bits)
        );
        scratch.fill(-1.0);
        // SAFETY: this test exclusively owns both output streams throughout the call.
        unsafe {
            layout.render(scratch.as_ptr());
        }
        for f in 0..2 {
            for c in 0..128 {
                let observed = if c < 64 {
                    first[f * 64 + c]
                } else {
                    second[f * 64 + c - 64]
                };
                let expected = if (offset as usize..offset as usize + 2).contains(&c) {
                    -1.0
                } else {
                    sample(f * 128 + c)
                };
                assert_eq!(observed.to_bits(), expected.to_bits());
            }
        }
        first.copy_from_slice(&before_first);
        second.copy_from_slice(&before_second);
    }
}

#[test]
fn rejected_transfer_preserves_both_caller_buffers() {
    let mut native = [7.0; 4];
    let mut scratch = [9.0; 4];
    let mut fixture = Fixture {
        count: 1,
        buffers: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: std::ptr::null_mut(),
        }; 64],
    };
    fixture.buffers[0] = AudioBuffer {
        channels: 2,
        byte_size: 15,
        data: native.as_mut_ptr(),
    };
    let selection = BufferSelection {
        channels: 2,
        offset: 0,
        total: 0,
        limit: 64,
    };
    for input in [false, true] {
        // SAFETY: complete metadata/caller storage; malformed bytes reject before PCM access.
        assert!(!unsafe {
            mr_device_buffers_transfer(
                (&raw const fixture).cast(),
                selection,
                scratch.as_mut_ptr(),
                input,
            )
        });
        assert_eq!(native.map(f32::to_bits), [7.0; 4].map(f32::to_bits));
        assert_eq!(scratch.map(f32::to_bits), [9.0; 4].map(f32::to_bits));
    }
    // SAFETY: missing metadata is explicitly rejected without dereference.
    assert_eq!(
        unsafe { mr_device_buffers_frames(std::ptr::null(), selection) },
        0
    );
}
