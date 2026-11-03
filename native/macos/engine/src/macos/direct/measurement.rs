use super::*;
use std::hint::black_box;

#[repr(C)]
struct List {
    count: u32,
    buffers: [AudioBuffer; 66],
}

fn list(pcm: &mut [f32], channels: usize, frames: usize, planar: bool) -> List {
    let mut list = List {
        count: u32::try_from(if planar { channels } else { 1 }).unwrap(),
        buffers: std::array::from_fn(|_| AudioBuffer {
            channels: 0,
            byte_size: 0,
            data: std::ptr::null_mut(),
        }),
    };
    let width = if planar { 1 } else { channels };
    for (buffer, samples) in list.buffers.iter_mut().zip(pcm.chunks_mut(frames * width)) {
        *buffer = AudioBuffer {
            channels: u32::try_from(width).unwrap(),
            byte_size: u32::try_from(std::mem::size_of_val(samples)).unwrap(),
            data: samples.as_mut_ptr(),
        };
    }
    list
}

fn exercise(input: &List, output: &List, map: &Binding, destination: &mut [f32], frames: u32) {
    // SAFETY: the fixture retains two separate PCM allocations and complete ABL
    // arrays for this synchronous call. Each has frames + 2 native frames.
    unsafe {
        Buffers::with_native(
            std::ptr::from_ref(input).cast(),
            std::ptr::from_ref(output).cast_mut().cast(),
            frames + 2,
            |mut buffers| {
                assert!(buffers.covers(map));
                buffers.transfer(map, 1, destination);
                buffers.write(map, 1, destination);
                buffers.write(map, 1, destination);
            },
        )
    }
    .unwrap();
    black_box(destination);
}

#[test]
fn direct_pcm_measurement() {
    if std::env::var_os("MIXRELAY_DIRECT_PCM_BENCH").is_none() {
        return;
    }
    for channels in [2, 8, 64] {
        for frames in [32_u32, 256] {
            for planar in [false, true] {
                for channel_offset in [0, 1] {
                    measure(channels, frames, planar, channel_offset);
                }
            }
        }
    }
}

fn measure(channels: usize, frames: u32, planar: bool, channel_offset: usize) {
    let native_frames = frames as usize + 2;
    let width = channels + channel_offset * 2;
    let mut input = vec![0.25_f32; native_frames * width];
    let mut output = vec![0.0_f32; input.len()];
    let input_list = list(&mut input, width, native_frames, planar);
    let output_list = list(&mut output, width, native_frames, planar);
    let mut destination = vec![0.0_f32; frames as usize * channels];
    let map = Binding {
        id: "measurement".into(),
        input: true,
        channels,
        offset: Some(channel_offset),
    };
    let mut durations = [0_u128; 7];
    for pass in 0..8 {
        let start = Instant::now();
        for _ in 0..2_000 {
            exercise(&input_list, &output_list, &map, &mut destination, frames);
        }
        if pass > 0 {
            durations[pass - 1] = start.elapsed().as_nanos() / 2_000;
        }
        assert!(
            destination
                .iter()
                .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
        );
    }
    durations.sort_unstable();
    assert!(
        output
            .iter()
            .any(|sample| sample.to_bits() == 8_000.0_f32.to_bits())
    );
    println!(
        "direct_pcm channels={channels} frames={frames} planar={planar} offset={channel_offset} median_ns={}",
        durations[3]
    );
    black_box(output);
}
