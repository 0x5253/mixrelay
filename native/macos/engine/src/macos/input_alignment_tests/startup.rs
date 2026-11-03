//! Native preparation must not become live FIFO latency or clock-rate error.
use super::*;

// 64 ms of capture while graph resources are still preparing, without wall-clock waits.
const PREPARE_BLOCKS: u32 = 24;

#[test]
fn first_graph_admission_drops_preparation_pcm_without_a_slow_clock_slew() {
    for capture_first in [true, false] {
        for batch in [1, 2, 4] {
            startup_case(capture_first, batch);
        }
    }
}

fn startup_case(capture_first: bool, batch: u32) {
    let resources = MacDevices::new(
        Arc::new(AdapterRuntimeStatus::new(
            adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )),
        48_000,
        128,
    );
    let input = NativeOwner::new(2, 48_000, true);
    let output = NativeOwner::new(2, 48_000, false);
    let devices = publish(&resources, &[("a", &input), ("out", &output)]);
    let _input_bridge = input.serve();
    let _output_bridge = output.serve();
    let mut processor = single_input_processor();
    let mut capture = Capture {
        io: MacEngineExternalIo::new(&devices),
        pcm: Vec::with_capacity(3_000 * 256),
    };
    let timebase = Timebase::current();
    for block in 0..PREPARE_BLOCKS + 3_000 {
        if capture_first || block < PREPARE_BLOCKS {
            capture_batch(&input, block, batch, timebase);
        }
        if block < PREPARE_BLOCKS {
            continue;
        }
        let graph_block = block - PREPARE_BLOCKS;
        tick_output(&output, timebase, graph_block);
        let measured = allocation_counter::measure(|| {
            processor
                .process_external_cycle(cycle(graph_block), &mut capture)
                .unwrap()
                .unwrap();
        });
        assert_eq!(measured.count_total, 0);
        if !capture_first {
            capture_batch(&input, block, batch, timebase);
        }
    }
    assert!(
        capture.pcm[..96_000 * 2 - 64]
            .iter()
            .all(|sample| sample.abs() < 1e-4),
        "PCM captured before graph admission leaked into the live route"
    );
    let peaks = events(&capture.pcm, 0);
    assert_eq!(
        peaks.len(),
        60,
        "capture_first={capture_first}, batch={batch}: missing startup impulses: {peaks:?}"
    );
    let delays: Vec<_> = peaks
        .iter()
        .enumerate()
        .map(|(index, &frame)| {
            i64::try_from(frame).unwrap() - i64::try_from(96_000 + index * 4_800).unwrap()
        })
        .collect();
    let excursion = delays.iter().max().unwrap() - delays.iter().min().unwrap();
    assert!(
        excursion <= 2,
        "capture_first={capture_first}, batch={batch}: equal clocks drifted after preparation: {delays:?}"
    );
}

fn capture_batch(input: &NativeOwner, block: u32, batch: u32, timebase: Timebase) {
    let count = if block < PREPARE_BLOCKS { 1 } else { batch };
    if block >= PREPARE_BLOCKS && !(block + 1 - PREPARE_BLOCKS).is_multiple_of(batch) {
        return;
    }
    let host = |frame| {
        timebase.ticks(Duration::from_nanos(
            1_000_000_000 + u64::from(frame) * 1_000_000_000 / 48_000,
        ))
    };
    for packet in block + 1 - count..=block {
        input.callback(
            &mut source_pcm(packet),
            CallbackTime {
                now: host((block + 1) * 128),
                host: host(packet * 128),
                sample: f64::from(packet * 128),
                rate_scalar: 1.0,
            },
        );
    }
}

fn single_input_processor() -> Box<BlockProcessor> {
    let document = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"a","type":"audio_device_input","config":{"selector":{"match":"persistent_uid","uid":"a"},"outputChannels":2}},
            {"id":"out","type":"audio_device_output","config":{"selector":{"match":"persistent_uid","uid":"out"},"inputChannels":2}}
        ],"edges":[
            {"id":"route","source":{"nodeId":"a","portId":"audio"},"destination":{"nodeId":"out","portId":"audio"}}
        ]
    })).unwrap();
    create_initial_processor(
        &document,
        RenderFormat {
            sample_rate: 48_000,
            block_frames: 128,
        },
        Arc::new(PeakMeterBank::new(8, 128)),
    )
    .unwrap()
}

fn source_pcm(block: u32) -> [f32; 256] {
    let first = block * 128;
    std::array::from_fn(|index| {
        if block < PREPARE_BLOCKS {
            return 0.75;
        }
        let frame = first + u32::try_from(index / 2).unwrap() - PREPARE_BLOCKS * 128;
        // Include the first Gaussian's leading half; a step at its peak would
        // introduce sinc ringing and multiple local maxima into the test signal.
        if frame < 96_000 - 64 {
            return 0.0;
        }
        let distance = (f64::from(frame) - 96_000.0 + 2_400.0).rem_euclid(4_800.0) - 2_400.0;
        #[expect(clippy::cast_possible_truncation, reason = "bounded synthetic PCM")]
        {
            (-(distance / 7.2).powi(2)).exp() as f32 * 0.25
        }
    })
}
