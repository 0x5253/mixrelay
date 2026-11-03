//! Unit contracts for catalog identity, topology, latency and instance ownership.

use crate::activation::check_creator_thread;
use crate::bus_topology::{
    BusObservation, channel_config, native_bus_layout, selected_channel_config_matches,
};
use crate::instantiation::AsyncInstantiationCompletion;
use crate::latency::{
    latency_frames_from_seconds, latency_property_changed, latency_snapshot,
    take_latency_invalidation, with_latency_invalidation,
};
use crate::latency_signal::acquire as acquire_latency_signal;
use crate::rendering::{BusPlaneRange, bus_plane_index, bus_plane_ranges};
use crate::scanner::{format_unique_id, four_cc, parse_four_cc, parse_unique_id};
use crate::stream_format::{planar_f32_format, render_format_matches};
use objc2_audio_toolbox::{
    AudioComponentDescription, kAudioUnitProperty_Latency, kAudioUnitScope_Global,
};
use objc2_core_audio_types::{
    kAudioChannelLayoutTag_AudioUnit_5_1, kAudioChannelLayoutTag_AudioUnit_7_1,
    kAudioChannelLayoutTag_Mono, kAudioChannelLayoutTag_Stereo, kAudioFormatFlagIsNonInterleaved,
};
use std::ptr;
use truce_rack_core::bus::{BusKind, ChannelConfig};

#[test]
fn four_cc_roundtrip() {
    let aufx = u32::from_be_bytes(*b"aufx");
    assert_eq!(four_cc(aufx), "aufx");
    assert_eq!(parse_four_cc("aufx"), Some(aufx));
}

#[test]
fn unique_id_roundtrip() {
    let desc = AudioComponentDescription {
        componentType: u32::from_be_bytes(*b"aufx"),
        componentSubType: u32::from_be_bytes(*b"dely"),
        componentManufacturer: u32::from_be_bytes(*b"appl"),
        componentFlags: 0,
        componentFlagsMask: 0,
    };
    let id = format_unique_id(&desc);
    assert_eq!(id, "aufx:dely:appl");
    let parsed = parse_unique_id(&id).unwrap();
    assert_eq!(parsed.componentType, desc.componentType);
    assert_eq!(parsed.componentSubType, desc.componentSubType);
    assert_eq!(parsed.componentManufacturer, desc.componentManufacturer);
}

#[test]
fn native_bus_layout_preserves_order_names_and_does_not_infer_sidechain() {
    let layout = native_bus_layout(
        BusObservation::new(
            &[2, 1],
            &[Some("Program".into()), Some("Key Trigger".into())],
            &[
                Some(kAudioChannelLayoutTag_Stereo),
                Some(kAudioChannelLayoutTag_Mono),
            ],
        ),
        BusObservation::new(
            &[2, 6],
            &[Some("Main Mix".into()), Some("Diagnostics".into())],
            &[
                Some(kAudioChannelLayoutTag_Stereo),
                Some(kAudioChannelLayoutTag_AudioUnit_5_1),
            ],
        ),
    )
    .unwrap();

    assert_eq!(layout.id, None);
    assert_eq!(layout.inputs.len(), 2);
    assert_eq!(layout.inputs[0].name, "Program");
    assert_eq!(layout.inputs[0].kind, BusKind::Main);
    assert_eq!(layout.inputs[0].channels, ChannelConfig::Stereo);
    assert_eq!(layout.inputs[1].name, "Key Trigger");
    assert_eq!(layout.inputs[1].kind, BusKind::Auxiliary);
    assert_eq!(layout.inputs[1].channels, ChannelConfig::Mono);
    assert_eq!(layout.outputs.len(), 2);
    assert_eq!(layout.outputs[0].name, "Main Mix");
    assert_eq!(layout.outputs[0].kind, BusKind::Main);
    assert_eq!(layout.outputs[0].channels, ChannelConfig::Stereo);
    assert_eq!(layout.outputs[1].name, "Diagnostics");
    assert_eq!(layout.outputs[1].kind, BusKind::Auxiliary);
    assert_eq!(layout.outputs[1].channels, ChannelConfig::Surround5_1);
}

#[test]
fn native_bus_layout_allows_no_input_and_rejects_unbounded_or_empty_output() {
    let generator = native_bus_layout(
        BusObservation::new(&[], &[], &[]),
        BusObservation::new(&[2], &[None], &[None]),
    )
    .unwrap();
    assert!(generator.inputs.is_empty());
    assert_eq!(generator.outputs[0].name, "Output");

    assert!(
        native_bus_layout(
            BusObservation::new(&[], &[], &[]),
            BusObservation::new(&[], &[], &[])
        )
        .is_err()
    );
    assert!(
        native_bus_layout(
            BusObservation::new(&[], &[], &[]),
            BusObservation::new(&[0], &[None], &[None])
        )
        .is_err()
    );
    assert!(
        native_bus_layout(
            BusObservation::new(&[], &[], &[]),
            BusObservation::new(&[u32::MAX], &[None], &[None])
        )
        .is_err()
    );
    assert!(
        native_bus_layout(
            BusObservation::new(&[1; 65], &[], &[]),
            BusObservation::new(&[1], &[None], &[None])
        )
        .is_err()
    );
    assert!(
        native_bus_layout(
            BusObservation::new(&[], &[], &[]),
            BusObservation::new(&[1; 65], &[], &[])
        )
        .is_err()
    );

    let unclassified = native_bus_layout(
        BusObservation::new(&[], &[], &[]),
        BusObservation::new(&[6], &[None], &[None]),
    )
    .unwrap();
    assert_eq!(unclassified.outputs[0].channels, ChannelConfig::Discrete(6));
}

#[test]
fn input_plane_ranges_route_native_buses_without_crossing_channels() {
    let ranges = bus_plane_ranges(&[2, 1, 6]).unwrap();
    assert_eq!(
        ranges,
        [
            BusPlaneRange { start: 0, len: 2 },
            BusPlaneRange { start: 2, len: 1 },
            BusPlaneRange { start: 3, len: 6 },
        ]
    );
    assert_eq!(bus_plane_index(&ranges, 0, 0), Some(0));
    assert_eq!(bus_plane_index(&ranges, 0, 1), Some(1));
    assert_eq!(bus_plane_index(&ranges, 1, 0), Some(2));
    assert_eq!(bus_plane_index(&ranges, 2, 5), Some(8));
    assert_eq!(bus_plane_index(&ranges, 0, 2), None);
    assert_eq!(bus_plane_index(&ranges, 3, 0), None);
}

#[test]
fn planar_f32_negotiation_requires_exact_channels_rate_and_packing() {
    let expected = planar_f32_format(48_000.0, 6);
    assert!(render_format_matches(&expected, &expected));

    let mut wrong_channels = expected;
    wrong_channels.mChannelsPerFrame = 2;
    assert!(!render_format_matches(&wrong_channels, &expected));

    let mut wrong_rate = expected;
    wrong_rate.mSampleRate = 44_100.0;
    assert!(!render_format_matches(&wrong_rate, &expected));

    let mut interleaved = expected;
    interleaved.mFormatFlags &= !kAudioFormatFlagIsNonInterleaved;
    assert!(!render_format_matches(&interleaved, &expected));
}

#[test]
fn standard_channel_configs_require_matching_native_layout_tags() {
    assert_eq!(
        channel_config(1, Some(kAudioChannelLayoutTag_Mono)),
        ChannelConfig::Mono
    );
    assert_eq!(
        channel_config(2, Some(kAudioChannelLayoutTag_Stereo)),
        ChannelConfig::Stereo
    );
    assert_eq!(
        channel_config(6, Some(kAudioChannelLayoutTag_AudioUnit_5_1)),
        ChannelConfig::Surround5_1
    );
    assert_eq!(
        channel_config(8, Some(kAudioChannelLayoutTag_AudioUnit_7_1)),
        ChannelConfig::Surround7_1
    );
    assert_eq!(channel_config(2, None), ChannelConfig::Discrete(2));
    assert_eq!(
        channel_config(6, Some(kAudioChannelLayoutTag_Stereo)),
        ChannelConfig::Discrete(6)
    );
    assert!(selected_channel_config_matches(
        ChannelConfig::Discrete(6),
        6,
        Some(kAudioChannelLayoutTag_AudioUnit_5_1)
    ));
    assert!(!selected_channel_config_matches(
        ChannelConfig::Surround5_1,
        6,
        None
    ));
}

#[test]
fn latency_seconds_round_up_to_frames_and_reject_invalid_values() {
    assert_eq!(latency_frames_from_seconds(0.0, 48_000.0), Some(0));
    assert_eq!(latency_frames_from_seconds(0.001, 48_000.0), Some(48));
    assert_eq!(latency_frames_from_seconds(0.000_001, 48_000.0), Some(1));
    for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
        for frames in 1..=4_096_u32 {
            let seconds = f64::from(frames) / sample_rate;
            assert_eq!(
                latency_frames_from_seconds(seconds, sample_rate),
                Some(frames),
                "{frames} frames at {sample_rate} Hz"
            );
        }
    }
    assert_eq!(
        latency_frames_from_seconds(1.01 / 48_000.0, 48_000.0),
        Some(2)
    );
    assert_eq!(
        latency_frames_from_seconds(1.5 / 48_000.0, 48_000.0),
        Some(2)
    );
    assert_eq!(latency_frames_from_seconds(f64::NAN, 48_000.0), None);
    assert_eq!(latency_frames_from_seconds(f64::INFINITY, 48_000.0), None);
    assert_eq!(latency_frames_from_seconds(-0.001, 48_000.0), None);
    assert_eq!(latency_frames_from_seconds(0.001, 0.0), None);
    assert_eq!(
        latency_frames_from_seconds(f64::from(u32::MAX) / 48_000.0, 48_000.0),
        Some(u32::MAX)
    );
    assert_eq!(
        latency_frames_from_seconds((f64::from(u32::MAX) + 1.0) / 48_000.0, 48_000.0),
        None
    );
    assert_eq!(
        latency_frames_from_seconds(1.0, f64::from(u32::MAX) + 1.0),
        None
    );
}

#[test]
fn latency_property_notification_only_invalidates_cached_snapshot() {
    let signal = acquire_latency_signal().unwrap();
    assert_eq!(latency_snapshot(Some(0), Some(&signal)), Some(0));
    assert_eq!(latency_snapshot(Some(0), None), None);

    // The callback receives only the opaque token and updates its validated static slot.
    unsafe {
        latency_property_changed(
            std::ptr::NonNull::new(signal.user_data()).unwrap(),
            ptr::null_mut(),
            kAudioUnitProperty_Latency,
            kAudioUnitScope_Global,
            0,
        );
    }
    assert_eq!(latency_snapshot(Some(64), Some(&signal)), None);
    assert!(take_latency_invalidation(Some(&signal)));
    assert!(!take_latency_invalidation(Some(&signal)));
    // The control service can publish its refreshed known zero without confusing it with
    // unknown; query failures remain None through the same snapshot boundary.
    assert_eq!(latency_snapshot(Some(0), Some(&signal)), Some(0));
    assert_eq!(latency_snapshot(None, Some(&signal)), None);
}

#[test]
fn failed_latency_changing_mutation_stays_unknown_without_notification() {
    let signal = acquire_latency_signal().unwrap();
    let mut cached = Some(64);
    let result = with_latency_invalidation(&mut cached, Some(&signal), || {
        // Invalidation precedes the native operation, even if it fails without notifying.
        assert!(signal.is_dirty());
        Err::<(), _>(-50)
    });

    assert_eq!(result, Err(-50));
    assert_eq!(cached, None);
    assert_eq!(latency_snapshot(cached, Some(&signal)), None);
}

#[test]
fn creator_thread_guard_rejects_control_access_from_another_thread() {
    let creator = std::thread::current().id();
    assert!(check_creator_thread(creator).is_ok());

    let error = std::thread::spawn(move || check_creator_thread(creator).unwrap_err().to_string())
        .join()
        .unwrap();
    assert_eq!(
        error,
        "Audio Unit control API called from a non-creator thread"
    );
}

#[test]
fn async_completion_transfers_success_and_failure_ownership_once() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let success = AsyncInstantiationCompletion::default();
    assert_eq!(success.complete(0x1234, 0, deadline), None);
    assert_eq!(success.take_completed(), Some((0x1234, 0)));
    assert_eq!(success.take_completed(), None);
    assert_eq!(success.abandon(), None);

    let failure = AsyncInstantiationCompletion::default();
    assert_eq!(failure.complete(0x5678, -50, deadline), None);
    assert_eq!(failure.take_completed(), Some((0x5678, -50)));
    // The caller owns this failed non-null instance and disposes it exactly once.
    assert_eq!(failure.abandon(), None);
}

#[test]
fn async_completion_timeout_and_callback_race_have_one_disposal_owner() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let timeout_wins = AsyncInstantiationCompletion::default();
    assert_eq!(timeout_wins.abandon(), None);
    assert_eq!(timeout_wins.complete(0x1234, 0, deadline), Some(0x1234));
    // Re-delivery of the same completion token does not hand the same unit out twice.
    assert_eq!(timeout_wins.complete(0x1234, 0, deadline), None);

    let callback_wins = AsyncInstantiationCompletion::default();
    assert_eq!(callback_wins.complete(0x5678, 0, deadline), None);
    // If completion wins the mutex race just before timeout, timeout takes and disposes
    // the result rather than leaving it owned by a callback that has already returned.
    assert_eq!(callback_wins.abandon(), Some(0x5678));
    assert_eq!(callback_wins.complete(0x5678, 0, deadline), None);

    let expired = AsyncInstantiationCompletion::default();
    let already_expired = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(expired.complete(0x9abc, 0, already_expired), Some(0x9abc));
    assert_eq!(expired.take_completed(), None);
    assert_eq!(expired.complete(0x9abc, 0, already_expired), None);
}
