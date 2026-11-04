use super::Device;
use crate::macos_devices::{
    device_fixture::synthetic_device, ffi, simulated::SimulatedHost, spec::Spec,
};
use mixrelay_audio_graph::DeviceSelector;
use mixrelay_audio_io::process::TransferOutcome;
use std::sync::atomic::Ordering;

#[test]
fn first_input_epoch_is_priming_but_later_shortages_remain_active() {
    use mixrelay_audio_io::process::InputActivity;
    let (device, context) = synthetic_device(1, 48_000, 48_000, true);
    let mut pcm = [9.0_f32; 32];
    // SAFETY: unpublished synthetic owner; the fixture serializes both ring
    // roles with full disjoint PCM spans and no native callback or worker.
    unsafe {
        assert_eq!(
            ffi::mr_device_io_bridge_write(device.native.pointer.as_ptr(), [0.25; 32].as_ptr(), 32,),
            32
        );
    }
    let allocation = allocation_counter::measure(|| {
        let first = device.read(&mut pcm, 0);
        assert_eq!(first.activity, InputActivity::Priming);
        assert_eq!(first.frames, 0);
        assert!(!first.discontinuity);
        assert!(pcm.iter().all(|sample| *sample == 0.0));
        let stalled = device.read(&mut pcm, 0);
        assert_eq!(stalled.activity, InputActivity::Active);
        assert_eq!(stalled.frames, 0);
    });
    assert_eq!(allocation.count_total, 0);
    // SAFETY: same sole synthetic producer after the reader's freshness reset.
    unsafe {
        assert_eq!(
            ffi::mr_device_io_bridge_write(device.native.pointer.as_ptr(), [0.75; 32].as_ptr(), 32,),
            32
        );
    }
    assert_eq!(device.read(&mut pcm, 0), TransferOutcome::complete(32));
    assert_eq!(pcm.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
    context.epoch.fetch_add(1, Ordering::Release);
    let restarted = device.read(&mut pcm, 0);
    assert_eq!(restarted.activity, InputActivity::Active);
    assert_eq!(restarted.frames, 0);
}

#[test]
fn wrong_direction_and_invalid_width_fail_before_callbacks() {
    for spec in [
        Spec {
            selector: DeviceSelector::DefaultOutput,
            input: true,
            channels: 2,
        },
        Spec {
            selector: DeviceSelector::DefaultInput,
            input: false,
            channels: 2,
        },
        Spec {
            selector: DeviceSelector::DefaultInput,
            input: true,
            channels: 65,
        },
    ] {
        assert!(Device::open(&spec, 48_000, SimulatedHost::new()).is_err());
    }
}
