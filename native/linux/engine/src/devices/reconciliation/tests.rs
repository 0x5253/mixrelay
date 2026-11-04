use super::*;
use mixrelay_audio_graph::{AudioDeviceInputConfig, AudioDeviceOutputConfig};
use mixrelay_audio_io::DeviceSelector;
use std::sync::atomic::Ordering;

fn watch() -> DeviceWatch {
    let spec = DeviceRequirements {
        inputs: vec![(
            "input".into(),
            AudioDeviceInputConfig {
                selector: DeviceSelector::PersistentUid { uid: "mic".into() },
                output_channels: 32,
                gain_db: 0.0,
            },
        )],
        outputs: vec![(
            "output".into(),
            AudioDeviceOutputConfig {
                selector: DeviceSelector::PersistentUid { uid: "sink".into() },
                input_channels: 32,
            },
        )],
    };
    let watch = DeviceWatch {
        owners: mixrelay_linux_pipewire::StreamOwners::default(),
        automatic_outputs: false,
        generation: 3,
        format: crate::runtime::render_format(),
        spec,
        inputs: BTreeMap::from([("input".into(), Arc::new(AtomicBool::new(true)))]),
        outputs: BTreeMap::from([("output".into(), Arc::new(AtomicBool::new(true)))]),
        input_selections: BTreeMap::from([(
            "input".into(),
            Arc::new(DeviceSelection::new("input", 10)),
        )]),
        output_selections: BTreeMap::from([(
            "output".into(),
            Arc::new(DeviceSelection::new("output", 20)),
        )]),
    };
    for selection in watch
        .input_selections
        .values()
        .chain(watch.output_selections.values())
    {
        selection.enabled.store(true, Ordering::Release);
    }
    watch
}

#[test]
fn retained_devices_need_no_registry_or_native_preparation() {
    let before = watch();
    let staged = DeviceStreams::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &before.spec,
        4,
        before.format,
        Some(&before),
    )
    .unwrap();
    assert_eq!(staged.generation, 3);
    assert_eq!((staged.inputs.len(), staged.outputs.len()), (1, 1));
    assert!(staged.inputs["input"].stream.is_none());
    assert!(Arc::ptr_eq(
        &staged.inputs["input"].connected,
        &before.inputs["input"]
    ));
    assert!(Arc::ptr_eq(
        &staged.outputs["output"].connected,
        &before.outputs["output"]
    ));
    before.inputs["input"].store(false, Ordering::Release);
    assert_eq!(
        staged
            .watch(before.spec.clone())
            .disconnected()
            .inputs
            .len(),
        1
    );
    assert!(!staged.can_retain_from(&DeviceStreams::default()));
    assert!(
        !staged.can_retain_from(&staged),
        "vacant slots are not active native owners"
    );
}

#[test]
fn abandoned_preparation_does_not_mutate_retained_execution_gates() {
    let before = watch();
    let cancelled = mixrelay_audio_graph::GraphDocument::new(4, vec![], vec![]);
    let staged = DeviceStreams::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &before.spec,
        4,
        before.format,
        Some(&before),
    )
    .unwrap();
    assert!(
        staged
            .inputs
            .values()
            .chain(staged.outputs.values())
            .all(|owner| owner.stream.is_none())
    );
    assert!(Arc::ptr_eq(
        &staged.inputs["input"].selected,
        &before.input_selections["input"]
    ));

    // Initialization precedes resource publication, which may reject or cancel.
    // The proposed document would disable these leases if they were overwritten.
    staged.initialize_controls(&cancelled);
    drop(staged);
    assert!(
        before.input_selections["input"]
            .enabled
            .load(Ordering::Acquire)
    );
    assert!(
        before.output_selections["output"]
            .enabled
            .load(Ordering::Acquire)
    );
}

#[test]
fn changed_identity_width_or_format_opens_only_required_devices() {
    let before = watch();
    for uid in ["mic", "different-mic"] {
        let mut next = before.spec.clone();
        next.inputs[0].1.selector = DeviceSelector::PersistentUid { uid: uid.into() };
        next.inputs[0].1.output_channels = if uid == "mic" { 64 } else { 32 };
        let (staged, required) = DeviceStreams::stage(&next, 4, before.format, Some(&before));
        assert_eq!(required.inputs, next.inputs);
        assert!(required.outputs.is_empty());
        assert!(staged.inputs.is_empty());
        assert_eq!(staged.outputs.len(), 1);
    }
    let (changed, required) = DeviceStreams::stage(
        &before.spec,
        5,
        RenderFormat {
            sample_rate: 44_100,
            block_frames: 256,
        },
        Some(&before),
    );
    assert_eq!(changed.generation, 5);
    assert_eq!(required, before.spec);
    assert!(changed.is_empty());
}

#[test]
fn removed_devices_leave_no_lease_and_readdition_requires_fresh_acquisition() {
    let before = watch();
    let removed = DeviceStreams::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &DeviceRequirements::default(),
        4,
        before.format,
        Some(&before),
    )
    .unwrap();
    assert!(removed.is_empty());
    let empty_watch = removed.watch(DeviceRequirements::default());
    let (readded, required) =
        DeviceStreams::stage(&before.spec, 5, before.format, Some(&empty_watch));
    assert_eq!(readded.generation, before.generation);
    assert!(readded.is_empty());
    assert_eq!(required, before.spec);
}

#[test]
fn recovery_inherits_original_lease_and_cancellation_never_queries_registry() {
    let original = watch();
    let different = watch();
    let (mut recovery, _) =
        DeviceStreams::stage(&different.spec, 4, different.format, Some(&different));
    assert!(!Arc::ptr_eq(
        &original.inputs["input"],
        &recovery.inputs["input"].connected
    ));
    recovery.inherit_leases(&original);
    assert!(Arc::ptr_eq(
        &original.inputs["input"],
        &recovery.inputs["input"].connected
    ));
    assert!(Arc::ptr_eq(
        &original.outputs["output"],
        &recovery.outputs["output"].connected
    ));
    original.inputs["input"].store(false, Ordering::Release);
    assert!(DeviceStreams::recover(&original, &AtomicBool::new(true)).is_err());
    assert!(
        DeviceStreams::recover(&different, &AtomicBool::new(false))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn healthy_defaults_are_observed_and_only_valid_changed_serials_request_replacement() {
    let mut before = watch();
    before.spec.inputs[0].1.selector = DeviceSelector::DefaultInput;
    before.spec.outputs[0].1.selector = DeviceSelector::DefaultOutput;
    assert!(before.needs_observation());
    for (id, selector, direction, current) in [
        (
            "input",
            DeviceSelector::DefaultInput,
            mixrelay_audio_io::DeviceDirection::Input,
            10,
        ),
        (
            "output",
            DeviceSelector::DefaultOutput,
            mixrelay_audio_io::DeviceDirection::Output,
            20,
        ),
    ] {
        assert!(!before.changed_default(id, &selector, direction, Some(current)));
        assert!(!before.changed_default(id, &selector, direction, None));
        assert!(before.changed_default(id, &selector, direction, Some(current + 1)));
    }
    assert!(before.inputs["input"].load(Ordering::Acquire));
    assert!(before.outputs["output"].load(Ordering::Acquire));
}

#[test]
fn persistent_removed_and_off_bindings_do_not_follow_defaults() {
    let before = watch();
    assert!(!before.needs_observation());
    assert!(!before.changed_default(
        "input",
        &before.spec.inputs[0].1.selector,
        mixrelay_audio_io::DeviceDirection::Input,
        Some(99)
    ));
    let removed = DeviceStreams::default().watch(DeviceRequirements::default());
    assert!(!removed.needs_observation());
    assert!(!removed.changed_default(
        "input",
        &DeviceSelector::DefaultInput,
        mixrelay_audio_io::DeviceDirection::Input,
        Some(99)
    ));
    let mut off = watch();
    off.spec.inputs[0].1.selector = DeviceSelector::DefaultInput;
    off.input_selections["input"]
        .enabled
        .store(false, Ordering::Release);
    off.inputs["input"].store(false, Ordering::Release);
    assert!(!off.needs_observation());
    assert!(!off.changed_default(
        "input",
        &DeviceSelector::DefaultInput,
        mixrelay_audio_io::DeviceDirection::Input,
        Some(99)
    ));
}

#[test]
fn queued_default_replacement_rejects_superseded_serial_and_reacquired_lease() {
    let before = watch();
    let (mut current, _) = DeviceStreams::stage(&before.spec, 3, before.format, Some(&before));
    let (mut queued, _) = DeviceStreams::stage(&before.spec, 3, before.format, Some(&before));
    let owner = current.inputs.get_mut("input").unwrap();
    let next = queued.inputs.get_mut("input").unwrap();
    owner.selected.serial.store(10, Ordering::Release);
    next.serial = 20;
    next.replaces = Some(10);
    assert!(owner.matches_recovery(next));
    assert!(owner.accepts_replacement(next));
    // Another accepted selection C fences a pending A-to-B result.
    owner.selected.serial.store(30, Ordering::Release);
    assert!(!owner.accepts_replacement(next));
    owner.selected.serial.store(10, Ordering::Release);
    owner.selected.enabled.store(false, Ordering::Release);
    assert!(!owner.accepts_replacement(next));
    owner.selected.enabled.store(true, Ordering::Release);
    next.connected = Arc::new(AtomicBool::new(true));
    assert!(!owner.matches_recovery(next));
    next.connected = owner.connected.clone();
    assert!(owner.matches_recovery(next));
    // An epoch change rejects even a matching serial and acquisition lease.
    let old = queued.generation;
    current.generation = old + 1;
    current.replace_disconnected(&mut queued);
    assert_eq!(
        current.inputs["input"]
            .selected
            .serial
            .load(Ordering::Acquire),
        10
    );
}
