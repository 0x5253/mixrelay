//! Native PCM rejection, dry fallback and bounded processing work.

use super::*;
use std::hint::black_box;

#[test]
fn native_output_events_use_the_prepared_budget_and_reject_overflow_without_allocating() {
    for count in [
        super::super::OUTPUT_EVENT_CAPACITY,
        super::super::OUTPUT_EVENT_CAPACITY + 1,
    ] {
        let mut plugin = fixture(vec![BusLayout::stereo()]);
        plugin.next_process_action = Some(FixtureProcessAction::OutputEvents(count));
        let mut worker = activate(plugin, 0, 4).unwrap();
        worker.input_storage[0].fill(0.25);
        worker.input_storage[1].fill(-0.5);
        let allocations = allocation_counter::measure(|| {
            let result = worker.process(4);
            if count == super::super::OUTPUT_EVENT_CAPACITY {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(super::super::ProcessFailure::Fatal)));
                worker.bypass(4);
            }
            assert_eq!(
                worker.output_events.len(),
                super::super::OUTPUT_EVENT_CAPACITY
            );
            assert_eq!(worker.output_storage[0], [0.25; 4]);
            assert_eq!(worker.output_storage[1], [-0.5; 4]);
        });
        assert_eq!(allocations.count_total, 0);
    }
}

#[test]
fn corrupt_vendor_pcm_is_rejected_before_publication_without_allocating() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut plugin = fixture(vec![BusLayout::stereo()]);
        plugin.latency_frames = Some(0);
        plugin.latency_after_process = Some(37);
        plugin.next_process_action = Some(FixtureProcessAction::NonFinite(value));
        let mut worker = activate(plugin, 0, 128).unwrap();
        worker.inputs_mut()[0][..4].copy_from_slice(&[0.25, -0.5, 0.0, 0.75]);
        worker.inputs_mut()[1][..4].copy_from_slice(&[-0.25, 0.5, 0.0, -0.75]);
        let allocation = allocation_counter::measure(|| {
            assert!(worker.process(4).is_err(), "vendor PCM must be rejected");
            assert_eq!(worker.intrinsic_latency_frames(), None);
            worker.bypass(4);
        });
        assert_eq!(allocation.count_total, 0);
        assert_eq!(&worker.outputs()[0][..4], &[0.25, -0.5, 0.0, 0.75]);
        assert_eq!(&worker.outputs()[1][..4], &[-0.25, 0.5, 0.0, -0.75]);
    }
}

#[test]
fn worker_faults_publish_aligned_dry_pcm_then_fresh_silence() {
    use crate::process_fault::{FaultKind, FaultObserver};
    for (action, kind) in [
        (
            FixtureProcessAction::NonFinite(f32::NAN),
            FaultKind::NonFinitePcm,
        ),
        (FixtureProcessAction::PartialFailure, FaultKind::NativeCall),
        (
            FixtureProcessAction::StatusErrorThenUnknown,
            FaultKind::ProcessStatus,
        ),
    ] {
        let directory = mixrelay_test_support::FixtureDir::new("worker-fault").unwrap();
        let path = directory.join("audio");
        let layout = crate::AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 4,
        };
        let (mut parent, _control) = crate::shared_transport::create(
            &path,
            layout,
            crate::instance_id::InstanceId::new(1).unwrap(),
        )
        .unwrap();
        let (mut shared, _worker_control) = crate::shared_transport::worker(&path).unwrap();
        let mut plugin = fixture(vec![BusLayout::stereo()]);
        plugin.next_process_action = Some(action);
        let mut plugin = activate(plugin, 0, 4).unwrap();
        let audible = [0.25, -0.5, 0.0, 0.75, -0.25, 0.5, 0.0, -0.75];
        let mut output = [9.0; 8];
        let allocations = allocation_counter::measure(|| {
            assert!(parent.submit(1, &audible, 4, || {}));
            crate::worker::dsp::process_packet(&mut shared, &mut plugin, layout);
            assert!(parent.receive(&mut output, 4, |_| {}));
            assert_eq!(output.map(f32::to_bits), audible.map(f32::to_bits));
            assert!(parent.submit(2, &[0.0; 8], 4, || {}));
            crate::worker::dsp::process_packet(&mut shared, &mut plugin, layout);
            assert!(parent.receive(&mut output, 4, |_| {}));
            assert_eq!(output.map(f32::to_bits), [0.0_f32; 8].map(f32::to_bits));
        });
        assert_eq!(allocations.count_total, 0);
        let mut observer = FaultObserver::default();
        let snapshot = observer.observe(&shared.header().process_faults);
        let mut deltas = snapshot.into_iter().flatten();
        let delta = deltas.next().unwrap();
        assert_eq!((delta.kind, delta.new_failures, delta.total), (kind, 1, 1));
        assert!(deltas.next().is_none());
        assert!(
            observer
                .observe(&shared.header().process_faults)
                .iter()
                .all(Option::is_none)
        );
    }
}

#[test]
fn partially_written_vendor_error_uses_complete_dry_pcm_without_allocating() {
    let mut plugin = fixture(vec![BusLayout::stereo()]);
    plugin.next_process_action = Some(FixtureProcessAction::PartialFailure);
    let mut worker = activate(plugin, 0, 128).unwrap();
    worker.inputs_mut()[0][..4].fill(0.25);
    worker.inputs_mut()[1][..4].fill(-0.5);
    let allocation = allocation_counter::measure(|| {
        assert!(worker.process(4).is_err());
        worker.bypass(4);
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(&worker.outputs()[0][..4], &[0.25; 4]);
    assert_eq!(&worker.outputs()[1][..4], &[-0.5; 4]);
}

#[test]
fn invalid_process_geometry_is_rejected_before_vendor_code() {
    let mut worker = activate(fixture(vec![BusLayout::stereo()]), 0, 128).unwrap();
    for frames in [0, 129, usize::MAX] {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker.process(frames)));
        assert!(outcome.is_ok(), "invalid dimensions must not panic");
        assert!(outcome.unwrap().is_err());
    }
}

#[test]
fn failed_worker_bypass_never_routes_sidechain_into_auxiliary_outputs() {
    use truce_rack_core::bus::{Bus, BusKind, ChannelConfig};
    for action in [
        FixtureProcessAction::StatusErrorThenUnknown,
        FixtureProcessAction::CallErrorThenUnknown,
    ] {
        let mut layout = BusLayout::stereo_with_sidechain("Key");
        layout.outputs.push(Bus {
            name: "Auxiliary".into(),
            kind: BusKind::Auxiliary,
            channels: ChannelConfig::Stereo,
        });
        let mut plugin = fixture(vec![layout]);
        plugin.next_process_action = Some(action);
        let mut worker = activate(plugin, 0, 32).unwrap();
        for (index, channel) in worker.input_storage.iter_mut().enumerate() {
            channel.fill(f32::from(u16::try_from(index + 1).unwrap()));
        }
        assert!(worker.process(7).is_err());
        for output in &mut worker.output_storage {
            output.fill(99.0);
        }
        let allocation = allocation_counter::measure(|| worker.bypass(7));
        assert_eq!(allocation.count_total, 0);
        assert_eq!(allocation.count_current, 0);
        for (index, output) in worker.output_storage.iter().enumerate() {
            let expected = if index < 2 {
                f32::from(u16::try_from(index + 1).unwrap())
            } else {
                0.0
            };
            assert!(
                output[..7]
                    .iter()
                    .all(|sample| sample.to_bits() == expected.to_bits())
            );
            assert!(
                output[7..]
                    .iter()
                    .all(|sample| sample.to_bits() == 99.0_f32.to_bits())
            );
        }
    }
}

#[test]
fn process_preserves_every_channel_at_supported_boundaries() {
    for channels in [1, 2, 8, 64] {
        let mut worker = worker(channels, 256);
        for frames in [1, 32, 256] {
            for output in &mut worker.output_storage {
                output.fill(0.0);
            }
            worker.process(frames).unwrap();
            for (input, output) in worker.input_storage.iter().zip(&worker.output_storage) {
                assert_eq!(&input[..frames], &output[..frames]);
                assert!(output[frames..].iter().all(|sample| *sample == 0.0));
            }
        }
    }
}

#[test]
fn process_channel_references_allocate_nothing() {
    for channels in [1, 2, 8, 64] {
        let mut worker = worker(channels, 256);
        let allocation = allocation_counter::measure(|| {
            for frames in [1, 32, 256] {
                worker.process(frames).unwrap();
            }
        });
        assert_eq!(allocation.count_total, 0);
    }
}

#[test]
fn process_channel_reference_measurement() {
    const ITERATIONS: u32 = 20_000;
    if std::env::var_os("MIXRELAY_MEASURE_RACK_CHANNELS").is_none() {
        return;
    }
    for channels in [2, 8, 64] {
        for frames in [32, 256] {
            let mut worker = worker(channels, frames);
            for _ in 0..ITERATIONS {
                black_box(&mut worker).process(black_box(frames)).unwrap();
            }
            let mut elapsed = [0.0; 7];
            for ns in &mut elapsed {
                let start = Instant::now();
                for _ in 0..ITERATIONS {
                    black_box(&mut worker).process(black_box(frames)).unwrap();
                }
                *ns = start.elapsed().as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS);
            }
            elapsed.sort_by(f64::total_cmp);
            for (input, output) in worker.input_storage.iter().zip(&worker.output_storage) {
                assert_eq!(input, output);
            }
            println!(
                "rack-process channels={channels} frames={frames} median_ns={:.3}",
                elapsed[3]
            );
        }
    }
}
