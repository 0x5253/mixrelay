//! Host IO spans can exceed the device's selectable callback quantum.
use super::{IoOperation, Transport, native_fixture, start};
use mixrelay_pcm_ring::SampleFormat;

#[test]
fn large_host_capture_preserves_received_quiet_and_signal_without_realtime_allocation() {
    for encoding in [SampleFormat::Float32, SampleFormat::Signed24] {
        capture_large_span(encoding);
    }
}

fn capture_large_span(encoding: SampleFormat) {
    let fixture = native_fixture();
    let transport = Transport::with_capacity(91, 2, 2, 16_384);
    let id = transport.add(&fixture, "Coalesced host IO", 48_000);
    assert_eq!(
        fixture.registry.set_format(id + 1, 48_000, encoding),
        super::OK
    );
    if encoding != SampleFormat::Float32 {
        assert_eq!(fixture.perform(), super::OK);
    }
    start(&fixture, id);
    let expected: Vec<f32> = (0_u16..8192)
        .flat_map(|frame| {
            let value = if frame < 4096 {
                0.0
            } else {
                f32::from(frame) / 16384.0
            };
            [value, -value]
        })
        .collect();
    for (index, block) in expected.chunks(512 * 2).enumerate() {
        let mut owned = block.to_vec();
        // SAFETY: the fixture retains its registry and owns 512 stereo Float32 frames.
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                device: id,
                stream: id + 2,
                render: true,
                client: 42,
                frame: index as u64 * 512,
                samples: owned.as_mut_ptr().cast(),
                frames: 512,
            })
        });
    }
    let mut storage = vec![9.0_f32; expected.len()];
    // SAFETY: initialized Float32 storage also owns this complete packed native byte extent.
    let captured = unsafe {
        std::slice::from_raw_parts_mut(
            storage.as_mut_ptr().cast::<u8>(),
            expected.len() * encoding.bytes_per_sample(),
        )
    };
    let measured = allocation_counter::measure(|| {
        // SAFETY: exclusive output contains the complete 8,192-frame stereo host span.
        assert!(
            unsafe {
                fixture.registry.io(IoOperation {
                    device: id,
                    stream: id + 1,
                    render: false,
                    client: 42,
                    frame: 0,
                    samples: captured.as_mut_ptr().cast(),
                    frames: 8192,
                })
            },
            "valid host span was rejected as a nominal buffer size"
        );
    });
    assert_eq!(measured.count_total, 0);
    let mut decoded = vec![0.0; expected.len()];
    assert!(encoding.decode(captured, &mut decoded));
    assert_eq!(decoded, expected);
}

#[test]
fn large_host_render_and_multiclient_capture_keep_each_tile_and_channel_once() {
    let fixture = native_fixture();
    let transport = Transport::with_capacity(92, 2, 3, 16_384);
    let id = transport.add(&fixture, "Coalesced mix", 48_000);
    start(&fixture, id);
    assert_eq!(fixture.registry.client(id, 43, true), super::OK);
    let mut first = [0.125_f32, -0.25].repeat(8192);
    let mut second: Vec<f32> = (0..8192)
        .flat_map(|frame| {
            if (2048..6144).contains(&frame) {
                [0.25, 0.125]
            } else {
                [0.0, 0.0]
            }
        })
        .collect();
    let mut captured = vec![9.0_f32; 8192 * 3];
    let measured = allocation_counter::measure(|| {
        for (client, samples) in [(42, first.as_mut_ptr()), (43, second.as_mut_ptr())] {
            // SAFETY: each exclusive input owns 8,192 stereo Float32 frames.
            assert!(unsafe {
                fixture.registry.io(IoOperation {
                    device: id,
                    stream: id + 2,
                    render: true,
                    client,
                    frame: 0,
                    samples: samples.cast(),
                    frames: 8192,
                })
            });
        }
        // SAFETY: output owns the entire 8,192-frame, three-channel native span.
        assert!(unsafe {
            fixture.registry.io(IoOperation {
                device: id,
                stream: id + 1,
                render: false,
                client: 42,
                frame: 0,
                samples: captured.as_mut_ptr().cast(),
                frames: 8192,
            })
        });
    });
    assert_eq!(measured.count_total, 0);
    for (frame, samples) in captured.chunks_exact(3).enumerate() {
        assert_eq!(
            samples,
            if (2048..6144).contains(&frame) {
                [0.375, -0.125, 0.0]
            } else {
                [0.125, -0.25, 0.0]
            }
        );
    }
}

#[test]
fn a_render_span_longer_than_the_queue_sizing_span_is_rejected_and_traced() {
    use mixrelay_macos_transport::client_pcm::MAX_RENDER_SPAN_FRAMES as SPAN;
    let fixture = native_fixture();
    let transport = Transport::new(93, 2, 2);
    let id = transport.add(&fixture, "Queue extent", 48_000);
    start(&fixture, id);
    let device = fixture.registry.pin(id).unwrap();
    device.io_trace.set_enabled(true);
    let frames = SPAN + 1;
    let mut samples = vec![0.25_f32; frames as usize * 2];
    let io = |render: bool, frames: u32, samples: &mut [f32]| {
        // SAFETY: each exclusive buffer owns `frames` stereo Float32 frames.
        unsafe {
            fixture.registry.io(IoOperation {
                device: id,
                stream: id + if render { 2 } else { 1 },
                render,
                client: 42,
                frame: 0,
                samples: samples.as_mut_ptr().cast(),
                frames,
            })
        }
    };
    // A longer span could be overwritten in its queue before ReadInput or the pump reads it.
    assert!(!io(true, frames, &mut samples));
    let counters = device.io_trace.admission.snapshot();
    let words = crate::io_admission::COUNTERS;
    assert_eq!(counters[words + 3], 1);
    assert_eq!(
        counters[words + 1],
        u64::from(frames) | (1 << 32) | (1 << 33)
    );
    assert!(io(true, SPAN, &mut samples));
    // Capture keeps the retention bound; unpublished frames are silence.
    assert!(io(false, frames, &mut samples));
    assert_eq!(device.io_trace.admission.snapshot()[words + 3], 1);
    let (published, unpublished) = samples.split_at(SPAN as usize * 2);
    assert!(
        published
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
    assert!(unpublished.iter().all(|sample| sample.to_bits() == 0));
}
