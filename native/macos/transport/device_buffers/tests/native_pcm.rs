use super::*;

fn selection(channels: u32) -> BufferSelection {
    BufferSelection {
        channels,
        offset: 0,
        total: 0,
        limit: 64,
    }
}

fn fixture(
    storage: &mut [u32],
    channels: u32,
    frames: usize,
    planar: bool,
    pcm: SampleFormat,
) -> Fixture {
    let mut fixture = Fixture {
        count: if planar { channels } else { 1 },
        buffers: [AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: std::ptr::null_mut(),
        }; 64],
    };
    for index in 0..fixture.count as usize {
        let width = if planar { 1 } else { channels as usize };
        let bytes = frames * width * pcm.bytes_per_sample();
        fixture.buffers[index] = AudioBuffer {
            channels: u32::try_from(width).unwrap(),
            byte_size: u32::try_from(bytes).unwrap(),
            data: storage
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(index * bytes)
                .cast(),
        };
    }
    fixture
}

#[test]
fn native_integer_layouts_convert_both_directions_without_callback_allocations() {
    // Even frame count keeps planar S16 planes aligned, with deliberately unaligned packed24.
    let frames = 6;
    for pcm in [
        SampleFormat::Signed16,
        SampleFormat::Signed24,
        SampleFormat::Signed32,
    ] {
        for channels in 1..=64_u32 {
            for planar in [false, true] {
                let length = frames * channels as usize;
                let source: Vec<f32> = (0..length)
                    .map(|index| (f32::from(u16::try_from(index % 7).unwrap()) - 3.0) / 8.0)
                    .collect();
                let mut scratch = vec![0.0; length];
                let mut storage = vec![0_u32; (length * pcm.bytes_per_sample()).div_ceil(4)];
                let fixture = fixture(&mut storage, channels, frames, planar, pcm);
                let list = (&raw const fixture).cast();
                let mut formats = NativeFormats::default();
                assert!(formats.append(channels, planar, pcm));
                assert!(!formats.all_float());
                let mut render = source.clone();
                let allocation = allocation_counter::measure(|| {
                    // SAFETY: complete aligned metadata, native byte extent and disjoint scratch.
                    unsafe {
                        assert_eq!(
                            native_frames(list, selection(channels), &formats),
                            u32::try_from(frames).unwrap()
                        );
                        assert!(native_transfer(
                            list,
                            selection(channels),
                            &formats,
                            render.as_mut_ptr(),
                            false
                        ));
                        assert!(native_transfer(
                            list,
                            selection(channels),
                            &formats,
                            scratch.as_mut_ptr(),
                            true
                        ));
                    }
                });
                assert_eq!(allocation.count_total, 0);
                assert_eq!(scratch, source);
            }
        }
    }
}

#[test]
fn mixed_native_codecs_preserve_selected_projection_and_unselected_bytes() {
    let mut integer = [0_u32; 3];
    let integer_bytes = integer.as_mut_ptr().cast::<u8>();
    // SAFETY: test owns 12 initialized bytes; packed24 uses exactly that extent.
    assert!(
        SampleFormat::Signed24.encode(&[0.25, 0.5, -0.25, -0.5], unsafe {
            slice::from_raw_parts_mut(integer_bytes, 12)
        })
    );
    let mut float = [0.75, -0.75];
    let buffers = [
        AudioBuffer {
            channels: 2,
            byte_size: 12,
            data: integer_bytes.cast(),
        },
        AudioBuffer {
            channels: 1,
            byte_size: 8,
            data: float.as_mut_ptr(),
        },
    ];
    let mut formats = NativeFormats::default();
    assert!(formats.append(2, false, SampleFormat::Signed24));
    assert!(formats.append(1, false, SampleFormat::Float32));
    let selection = BufferSelection {
        channels: 2,
        offset: 1,
        total: 3,
        limit: 2048,
    };
    let layout = Layout::validate_native(&buffers, selection, &formats).unwrap();
    let mut scratch = [0.0; 4];
    // SAFETY: retained two-stream metadata, disjoint scratch and exclusive native output.
    unsafe {
        layout.capture(scratch.as_mut_ptr());
    }
    assert_eq!(
        scratch.map(f32::to_bits),
        [0.5_f32, 0.75, -0.5, -0.75].map(f32::to_bits)
    );
    scratch.fill(0.125);
    // SAFETY: same complete disjoint owned spans, with output mutation now exclusive.
    unsafe {
        layout.render(scratch.as_ptr());
    }
    let mut decoded = [0.0; 4];
    // SAFETY: native mutation has ended; only initialized retained bytes are borrowed now.
    assert!(SampleFormat::Signed24.decode(
        unsafe { slice::from_raw_parts(integer_bytes, 12) },
        &mut decoded
    ));
    assert_eq!(
        decoded.map(f32::to_bits),
        [0.25_f32, 0.125, -0.25, 0.125].map(f32::to_bits)
    );
    assert_eq!(float.map(f32::to_bits), [0.125_f32; 2].map(f32::to_bits));
}

#[test]
fn native_layout_rejects_changed_codec_geometry_before_touching_pcm() {
    let mut storage = [0_u32; 8];
    let mut fixture = fixture(&mut storage, 2, 4, false, SampleFormat::Signed16);
    let mut formats = NativeFormats::default();
    assert!(formats.append(2, false, SampleFormat::Signed16));
    let mut scratch = [9.0; 8];
    for (channels, bytes, offset) in [
        (1, 16, 0),
        (2, 15, 0),
        (2, 16, 1),
        (2, 0, 0),
        (2, 4097 * 4, 0),
    ] {
        fixture.buffers[0] = AudioBuffer {
            channels,
            byte_size: bytes,
            data: storage
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(offset)
                .cast(),
        };
        // SAFETY: malformed metadata rejects before borrowing the declared PCM extent.
        assert!(!unsafe {
            native_transfer(
                (&raw const fixture).cast(),
                selection(2),
                &formats,
                scratch.as_mut_ptr(),
                true,
            )
        });
        assert_eq!(scratch.map(f32::to_bits), [9.0_f32; 8].map(f32::to_bits));
        assert_eq!(storage, [0; 8]);
    }
}
