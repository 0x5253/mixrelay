use super::DeviceSet;
use crate::macos_devices::{
    bridge::DeviceBridge,
    device_fixture::{synthetic_device, synthetic_output},
    spec::Spec,
};
use mixrelay_audio_graph::DeviceSelector;
use std::{collections::BTreeMap, sync::atomic::Ordering};

#[test]
fn duplex_driver_binding_lookup_keeps_capture_and_render_widths_without_rt_allocation() {
    let selector = DeviceSelector::PersistentUid {
        uid: mixrelay_macos_transport::endpoint_identity::endpoint_uid([1; 16]),
    };
    let devices = DeviceSet(
        BTreeMap::new(),
        BTreeMap::from([
            (
                "driver/capture".into(),
                Spec {
                    selector: selector.clone(),
                    input: true,
                    channels: 3,
                },
            ),
            (
                "driver/render".into(),
                Spec {
                    selector,
                    input: false,
                    channels: 7,
                },
            ),
        ]),
    );
    let mut input = [9.0; 96];
    let output = [0.5; 224];
    let allocation = allocation_counter::measure(|| {
        assert_eq!(devices.binding_key("driver", true), Some("driver/capture"));
        assert_eq!(devices.binding_key("driver", false), Some("driver/render"));
        assert_eq!(devices.read("driver", &mut input, 0).unwrap().frames, 32);
        assert_eq!(devices.write("driver", &output, 0).unwrap().frames, 32);
        assert!(input.iter().all(|sample| *sample == 0.0));
        assert_eq!(devices.read("missing", &mut input, 0), None);
        assert_eq!(devices.write("missing", &output, 0), None);
    });
    assert_eq!(allocation.count_total, 0);
}

#[test]
fn processing_admission_waits_for_native_rate_and_rebuilt_bridge() {
    let (device, context) = synthetic_output(2, 96_000, 48_000);
    let devices = DeviceSet(BTreeMap::from([("output".into(), device)]), BTreeMap::new());
    assert!(
        !devices.processing_ready(96_000),
        "saved rate is not native proof"
    );
    assert!(
        !devices.processing_ready(48_000),
        "plan must match the prepared bridge"
    );
    context.graph_rate.store(48_000, Ordering::Release);
    assert!(
        !devices.processing_ready(48_000),
        "old bridge must not activate"
    );
    context.ready_rate.store(48_000, Ordering::Release);
    assert!(devices.processing_ready(48_000));
    context.native_rate.store(96_000, Ordering::Release);
    assert!(
        !devices.processing_ready(48_000),
        "reconnection must revalidate rate"
    );
    context.graph_rate.store(96_000, Ordering::Release);
    context.ready_rate.store(0, Ordering::Release);
    assert!(!devices.processing_ready(96_000));
    context.ready_rate.store(96_000, Ordering::Release);
    assert!(devices.processing_ready(96_000));
}

#[test]
fn admission_keeps_independent_secondary_conversion_and_offline_branches() {
    let (master, _) = synthetic_output(2, 48_000, 48_000);
    let (secondary, secondary_context) = synthetic_output(2, 48_000, 44_100);
    let devices = DeviceSet(
        BTreeMap::from([("a-output".into(), master), ("b-output".into(), secondary)]),
        BTreeMap::new(),
    );
    assert!(devices.processing_ready(48_000));
    secondary_context.failed.store(true, Ordering::Release);
    assert!(devices.processing_ready(48_000));
    assert!(DeviceSet::default().processing_ready(96_000));
}

#[test]
fn split_pacing_burst_retains_the_complete_callback_not_one_dsp_block() {
    for (native_rate, aligned) in [(48_000, true), (96_000, false)] {
        let mut transferred = Vec::new();
        for pacing_quantum in [32, 256] {
            let (device, context) = synthetic_device(1, 48_000, native_rate, true);
            context.clock_aligned.store(aligned, Ordering::Release);
            let devices = DeviceSet(
                BTreeMap::from([("input".into(), device.clone())]),
                BTreeMap::new(),
            );
            devices.set_graph_quantum(pacing_quantum);
            let mut bridge = DeviceBridge::new(&context, native_rate).unwrap();
            // Establish the graph consumer epoch before queuing this callback.
            let mut pcm = [0.0; 32];
            device.read(&mut pcm, 0);
            for _ in 0..4 {
                bridge.bridge.push_interleaved(&[0.25; 256]).unwrap();
            }
            // Let sinc startup consume its filter history before checking
            // the retained steady-state transfer window.
            for _ in 0..4 {
                bridge.pump(&context).unwrap();
            }
            let frames: u32 = (0..8).map(|_| device.read(&mut pcm, 0).frames).sum();
            let expected = if aligned {
                pacing_quantum
            } else {
                (pacing_quantum * 2)
                    .max(u32::try_from(bridge.bridge.processing_quantum().1 * 2).unwrap())
            }
            .min(256)
                / 32
                * 32;
            assert_eq!(frames, expected);
            transferred.push(frames);
        }
        assert_eq!(transferred[1], 256);
        eprintln!(
            "{native_rate}->48000 Hz, retained PCM for 32/256-frame callbacks: {transferred:?}"
        );
    }
}

#[test]
fn disabled_pcm_does_not_poison_sync_or_replace_the_hardware_clock() {
    let (device, context) = synthetic_output(2, 48_000, 48_000);
    let devices = DeviceSet(
        BTreeMap::from([("output".into(), device.clone())]),
        BTreeMap::new(),
    );
    let owner = devices.hardware_clock_progress().unwrap().owner_id;
    assert!(!devices.is_binding_inactive("output", false));
    device.set_active(false);
    let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
    assert!(!bridge.synchronize_activity(&context));
    assert!(devices.is_binding_inactive("output", false));
    assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
    device.set_active(true);
    assert!(!devices.is_binding_inactive("output", false));
    assert_eq!(devices.clock_synchronized("output", false), Some(false));
    assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
}

#[test]
fn same_native_device_bypasses_src_but_unknown_and_other_clocks_do_not() {
    let (master, _) = synthetic_output(2, 48_000, 48_000);
    let (peer, peer_context) = synthetic_output(2, 48_000, 48_000);
    let (other, other_context) = synthetic_output(2, 48_000, 48_000);
    master.clock_identity.store(41, Ordering::Release);
    peer.clock_identity.store(41, Ordering::Release);
    other.clock_identity.store(42, Ordering::Release);
    let devices = DeviceSet(
        BTreeMap::from([
            ("master".into(), master.clone()),
            ("peer".into(), peer.clone()),
            ("other".into(), other),
        ]),
        BTreeMap::new(),
    );
    // Reproduce the previous blanket-independent policy under exactly the
    // same channel/rate/input conditions as the new relationship decision.
    let mut before = DeviceBridge::new(&peer_context, 48_000).unwrap();
    let source = [0.25; 1024];
    let mut output = [0.0; 512];
    before.bridge.push_interleaved(&source[..512]).unwrap();
    before.bridge.push_interleaved(&source[..512]).unwrap();
    let previous = before.bridge.pull_interleaved(&mut output).unwrap();
    devices.set_clock_master(Some(master.identity));
    assert!(peer_context.clock_aligned.load(Ordering::Acquire));
    assert!(!other_context.clock_aligned.load(Ordering::Acquire));
    let mut after = DeviceBridge::new(&peer_context, 48_000).unwrap();
    after.bridge.push_interleaved(&source[..512]).unwrap();
    after.bridge.push_interleaved(&source[..512]).unwrap();
    let current = after.bridge.pull_interleaved(&mut output).unwrap();
    assert!(previous.algorithmic_delay_frames > 0);
    assert_eq!(current.algorithmic_delay_frames, 0);
    assert_eq!(current.transferred_frames, 256);
    assert!(
        output
            .iter()
            .all(|sample| sample.to_bits() == 0.25_f32.to_bits())
    );
    eprintln!(
        "same-device 2ch/48000Hz/256-frame pull: SRC delay {} -> {} frames, bypass PCM bit-exact",
        previous.algorithmic_delay_frames, current.algorithmic_delay_frames
    );

    // Zero means unverified (including user aggregates), not shared clock.
    master.clock_identity.store(0, Ordering::Release);
    peer.clock_identity.store(0, Ordering::Release);
    devices.set_clock_master(Some(master.identity));
    assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
    master.clock_identity.store(41, Ordering::Release);
    peer.clock_identity.store(41, Ordering::Release);
    peer.native_rate.store(44_100, Ordering::Release);
    devices.set_clock_master(Some(master.identity));
    assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
    peer.native_rate.store(48_000, Ordering::Release);
    master.failed.store(true, Ordering::Release);
    devices.set_clock_master(Some(master.identity));
    assert!(!peer_context.clock_aligned.load(Ordering::Acquire));
    devices.set_clock_master(None);
    assert!(!master.clock_aligned.load(Ordering::Acquire));
}

#[test]
fn hardware_master_identity_survives_bridge_priming_and_epoch_changes() {
    let (device, context) = synthetic_output(2, 48_000, 48_000);
    let devices = DeviceSet(
        BTreeMap::from([("output".into(), device.clone())]),
        BTreeMap::new(),
    );
    let owner = devices.hardware_clock_progress().unwrap().owner_id;
    devices.set_clock_master(Some(owner));
    assert!(context.clock_aligned.load(Ordering::Acquire));
    device.ready_rate.store(0, Ordering::Release);
    device.epoch.fetch_add(1, Ordering::Release);
    assert_eq!(devices.hardware_clock_progress().unwrap().owner_id, owner);
    devices.set_clock_master(Some(owner));
    assert!(context.clock_aligned.load(Ordering::Acquire));
    let mut bridge = DeviceBridge::new(&context, 48_000).unwrap();
    assert!(bridge.clock_aligned);
    device.ready_rate.store(48_000, Ordering::Release);
    let source = [0.25; 1024];
    assert_eq!(device.write(&source, 0).frames, 512);
    for _ in 0..4 {
        bridge.pump(&context).unwrap();
    }
    bridge.observe(&context, true).unwrap();
    assert_eq!(
        bridge.bridge.correction_ppm().to_bits(),
        0.0_f64.to_bits(),
        "native master cannot fight its own pacing PI"
    );
    devices.set_clock_master(None);
    assert!(!context.clock_aligned.load(Ordering::Acquire));
    device.failed.store(true, Ordering::Release);
    assert!(devices.hardware_clock_progress().is_none());
}
