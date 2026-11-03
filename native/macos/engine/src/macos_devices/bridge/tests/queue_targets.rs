use super::*;
use std::{fs::OpenOptions, hint::black_box, time::Instant};

#[test]
fn observed_callback_bursts_preserve_targets_at_one_and_32_channels() {
    for channels in [1_u16, 32] {
        for (input, initial) in [(false, [532, 256]), (true, [256, 148])] {
            for aligned in [false, true] {
                let (_device, context) =
                    crate::macos_devices::tests::synthetic_device(channels, 48_000, 96_000, input);
                context.clock_aligned.store(aligned, Ordering::Release);
                let bridge = DeviceBridge::new(&context, 96_000).unwrap();
                let divisor = if aligned { 2 } else { 1 };
                let targets = || {
                    [
                        bridge.queue_target(&context, false),
                        bridge.queue_target(&context, true),
                    ]
                };
                assert_eq!(targets(), initial.map(|frames| frames / divisor));

                let mut pcm = vec![0.25_f32; 512 * usize::from(channels)];
                let mut native = InputBuffer {
                    count: 1,
                    buffers: [AudioBuffer {
                        channels: u32::from(channels),
                        byte_size: u32::try_from(pcm.len() * 4).unwrap(),
                        data: pcm.as_mut_ptr(),
                    }],
                };
                native_callback(&context, &mut native);
                assert_eq!(targets(), [1024 / divisor, 512 / divisor]);
                context.graph_quantum.store(2048, Ordering::Release);
                assert_eq!(targets(), [8192 / divisor, 4096 / divisor]);
                context.graph_quantum.store(16_384, Ordering::Release);
                assert_eq!(targets(), [8192, 8192]);
            }
        }
    }
}

#[derive(serde::Serialize)]
struct Measurement {
    native_rate: u32,
    graph_rate: u32,
    input: bool,
    aligned: bool,
    native_quantum: u32,
    graph_quantum: u32,
    targets: [usize; 2],
    calls: usize,
    elapsed_ns: u128,
    allocations: u64,
}

#[test]
#[ignore = "serial release characterization; needs a fresh MIXRELAY_QUEUE_TARGET_OUTPUT path"]
fn measure_queue_targets() {
    let path = std::env::var("MIXRELAY_QUEUE_TARGET_OUTPUT").unwrap();
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap();
    let mut rows = Vec::new();
    for (native_rate, graph_rate) in [
        (48_000, 48_000),
        (96_000, 48_000),
        (44_100, 48_000),
        (48_000, 96_000),
        (8_000, 384_000),
        (384_000, 8_000),
    ] {
        for input in [false, true] {
            for aligned in [false, true] {
                rows.extend(measure_geometry(native_rate, graph_rate, input, aligned));
            }
        }
    }
    serde_json::to_writer_pretty(output, &rows).unwrap();
    assert!(rows.iter().all(|row| row.allocations == 0));
}

fn measure_geometry(
    native_rate: u32,
    graph_rate: u32,
    input: bool,
    aligned: bool,
) -> Vec<Measurement> {
    let (_device, context) =
        crate::macos_devices::tests::synthetic_device(1, graph_rate, native_rate, input);
    context.clock_aligned.store(aligned, Ordering::Release);
    let bridge = DeviceBridge::new(&context, native_rate).unwrap();
    let mut pcm = [0.25_f32; 2048];
    let mut native = InputBuffer {
        count: 1,
        buffers: [AudioBuffer {
            channels: 1,
            byte_size: 0,
            data: pcm.as_mut_ptr(),
        }],
    };
    let mut rows = Vec::new();
    for native_quantum in [0, 32, 128, 512, 2048] {
        if native_quantum != 0 {
            native.buffers[0].byte_size = native_quantum * 4;
            native_callback(&context, &mut native);
        }
        for graph_quantum in [0, 32, 128, 256, 2048, 16_384] {
            context
                .graph_quantum
                .store(graph_quantum, Ordering::Release);
            for _ in 0..1000 {
                black_box(bridge.queue_target(black_box(&context), black_box(true)));
                black_box(bridge.queue_target(black_box(&context), black_box(false)));
            }
            let targets = [
                bridge.queue_target(&context, false),
                bridge.queue_target(&context, true),
            ];
            let started = Instant::now();
            let allocations = allocation_counter::measure(|| {
                for _ in 0..10_000 {
                    black_box(bridge.queue_target(black_box(&context), black_box(true)));
                    black_box(bridge.queue_target(black_box(&context), black_box(false)));
                }
            });
            rows.push(Measurement {
                native_rate,
                graph_rate,
                input,
                aligned,
                native_quantum,
                graph_quantum,
                targets,
                calls: 20_000,
                elapsed_ns: started.elapsed().as_nanos(),
                allocations: allocations.count_total,
            });
        }
    }
    rows
}
