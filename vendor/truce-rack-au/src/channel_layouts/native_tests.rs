use crate::bus_topology::discover_bus_layout;
use crate::{AuPlugin, AuScanner};
use objc2_audio_toolbox::{
    AudioUnitSetProperty, kAudioUnitProperty_BypassEffect, kAudioUnitScope_Global,
};
use truce_rack_core::{
    buffer::{AudioBuffer, BusRange},
    events::EventList,
    plugin::{Plugin, PluginCore, ProcessContext, ProcessStatus},
    scanner::PluginScanner,
};

fn limiter_info() -> truce_rack_core::info::PluginInfo {
    let scanner = AuScanner::new();
    scanner
        .scan()
        .unwrap()
        .into_iter()
        .find(|info| info.name.contains("AUPeakLimiter"))
        .expect("macOS ships AUPeakLimiter")
}

fn load_limiter() -> AuPlugin {
    AuScanner::new().load(&limiter_info()).unwrap()
}

fn assert_bypass_pcm(plugin: &mut AuPlugin, channels: usize) {
    let bypass = 1_u32;
    // SAFETY: this test exclusively owns the live AU; the property reads one UInt32.
    assert_eq!(
        unsafe {
            AudioUnitSetProperty(
                plugin.instance,
                kAudioUnitProperty_BypassEffect,
                kAudioUnitScope_Global,
                0,
                (&raw const bypass).cast(),
                4,
            )
        },
        0
    );
    let inputs: Vec<Vec<f32>> = (0..channels)
        .map(|channel| {
            (0..128_u16)
                .map(|frame| {
                    (f32::from(u16::try_from(channel).unwrap()) + 1.0) * f32::from(frame) / 32768.0
                })
                .collect()
        })
        .collect();
    let mut outputs = vec![vec![f32::NAN; 128]; channels];
    let input_refs: Vec<&[f32]> = inputs.iter().map(Vec::as_slice).collect();
    let mut output_refs: Vec<&mut [f32]> = outputs.iter_mut().map(Vec::as_mut_slice).collect();
    let ranges = [BusRange::new(0, channels)];
    let mut buffer = AudioBuffer::new(&input_refs, &mut output_refs, 128, &ranges, &ranges);
    let mut output_events = EventList::new();
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: 128,
        transport: None,
        output_events: &mut output_events,
    };
    assert_eq!(
        plugin
            .process(&mut buffer, &EventList::new(), &mut context)
            .unwrap(),
        ProcessStatus::Continue
    );
    assert_eq!(
        outputs, inputs,
        "every channel must reach its matching output exactly"
    );
}

#[test]
fn apple_limiter_alternative_widths_restore_topology_and_render_native_pcm() {
    let mut plugin = load_limiter();
    // SAFETY: exclusive access to the newly loaded, inactive AU.
    let original = unsafe { discover_bus_layout(plugin.instance) }.unwrap().0;
    assert_eq!(plugin.supported_layouts()[0], original);
    let catalog = plugin.supported_layouts().to_vec();
    for channels in [1_u32, 2, 8, 32, 64, 2, 1] {
        let layout = catalog
            .iter()
            .find(|layout| {
                layout.total_input_channels() == channels
                    && layout.total_output_channels() == channels
            })
            .unwrap_or_else(|| panic!("missing {channels} channels from {catalog:?}"))
            .clone();
        plugin.activate(layout.clone(), 48_000.0, 128).unwrap();
        assert_eq!(plugin.active_layout(), Some(&layout));
        assert_bypass_pcm(&mut plugin, usize::try_from(channels).unwrap());
        plugin.deactivate();
        assert!(!plugin.is_active());
        assert_eq!(plugin.render.render_ctx.as_deref().unwrap().frames, 0);
        assert_eq!(plugin.supported_layouts(), catalog);
    }
    let fresh = load_limiter();
    assert_eq!(
        fresh.supported_layouts(),
        catalog,
        "discovery must be deterministic across instances"
    );
}

#[test]
fn unadvertised_layout_is_rejected_without_mutating_native_formats() {
    let mut plugin = load_limiter();
    let mut invalid = plugin.supported_layouts()[0].clone();
    invalid.outputs[0].channels = truce_rack_core::bus::ChannelConfig::Discrete(65);
    // SAFETY: test owns the inactive native instance exclusively.
    let before = unsafe { discover_bus_layout(plugin.instance) }.unwrap();
    assert!(plugin.activate(invalid, 48_000.0, 128).is_err());
    let after = unsafe { discover_bus_layout(plugin.instance) }.unwrap();
    assert_eq!(before, after);
    assert!(!plugin.is_active());
}

#[test]
fn selected_reload_keeps_formats_untouched_until_activation() {
    let info = limiter_info();
    let scanner = AuScanner::new();
    let catalog_owner = scanner.load(&info).unwrap();
    // SAFETY: exclusively owned, inactive test instance; no callbacks are running.
    let original = unsafe { discover_bus_layout(catalog_owner.instance) }.unwrap();
    for channels in [1, 2, 8, 32, 64] {
        let selected = catalog_owner
            .supported_layouts()
            .iter()
            .find(|layout| {
                layout.total_input_channels() == channels
                    && layout.total_output_channels() == channels
            })
            .unwrap();
        let mut worker = scanner.load_with_layout(&info, selected).unwrap();
        assert_eq!(worker.supported_layouts(), std::slice::from_ref(selected));
        // SAFETY: worker has not activated; selected-load must only query the native instance.
        assert_eq!(
            unsafe { discover_bus_layout(worker.instance) }.unwrap(),
            original
        );
        worker.activate(selected.clone(), 48_000.0, 128).unwrap();
        assert_bypass_pcm(&mut worker, usize::try_from(channels).unwrap());
        worker.deactivate();
        assert_eq!(worker.render.render_ctx.as_deref().unwrap().frames, 0);
    }
}

#[test]
fn selected_reload_rejects_changed_bus_roles_and_unbounded_channels() {
    let info = limiter_info();
    let scanner = AuScanner::new();
    let owner = scanner.load(&info).unwrap();
    let mut selected = owner.supported_layouts()[0].clone();
    selected.inputs[0].kind = truce_rack_core::bus::BusKind::Sidechain;
    assert!(scanner.load_with_layout(&info, &selected).is_err());
    selected.inputs[0].kind = truce_rack_core::bus::BusKind::Main;
    selected.inputs[0].channels = truce_rack_core::bus::ChannelConfig::Discrete(65);
    assert!(scanner.load_with_layout(&info, &selected).is_err());
}

#[test]
fn initialized_readback_rejects_geometry_changes_and_restores_reusable_instance() {
    let mut plugin = load_limiter();
    let layout = plugin.supported_layouts()[0].clone();
    // SAFETY: the test exclusively owns an inactive AU on its creator thread.
    let previous = unsafe {
        crate::stream_format::capture_native_formats(
            plugin.instance,
            &plugin.input_bus_channels,
            &plugin.output_bus_channels,
        )
    }
    .unwrap();
    for changed_count in [true, false] {
        // Match the negotiated rate first so a geometry rejection cannot be masked by
        // the factory default sample rate.
        unsafe {
            crate::stream_format::apply_and_verify_native_formats(
                plugin.instance,
                &plugin.input_bus_channels,
                &plugin.output_bus_channels,
                &layout,
                48_000.0,
            )
        }
        .unwrap();
        let mut mismatched = layout.clone();
        if changed_count {
            mismatched.outputs.push(mismatched.outputs[0].clone());
        } else {
            mismatched.outputs[0].channels = truce_rack_core::bus::ChannelConfig::Discrete(64);
        }
        // Exercise the post-initialize mismatch branch against the real native unit.
        // No mismatched layout is ever published to the render path.
        let error = plugin
            .initialize_selected_layout(&mismatched, 48_000.0, &previous)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if changed_count {
                "bus count changed"
            } else {
                "did not accept planar f32"
            }),
            "{error}"
        );
        assert!(!plugin.initialized);
        assert!(!plugin.is_active());
        assert!(plugin.render.render_ctx.is_none());
        assert!(plugin.render.host_transport.is_none());
        for format in &previous {
            assert_eq!(
                unsafe {
                    crate::bus_topology::read_stream_format(
                        plugin.instance,
                        format.scope,
                        format.element,
                    )
                }
                .unwrap(),
                format.format
            );
        }
    }
    plugin.activate(layout.clone(), 48_000.0, 128).unwrap();
    assert_bypass_pcm(
        &mut plugin,
        usize::try_from(layout.total_output_channels()).unwrap(),
    );
    plugin.deactivate();
    assert!(!plugin.initialized);
}

#[test]
fn incomplete_uninitialization_blocks_stateful_reactivation_before_native_mutation() {
    let mut plugin = load_limiter();
    let layout = plugin.supported_layouts()[0].clone();
    plugin.activate(layout.clone(), 48_000.0, 128).unwrap();
    let state = plugin.save_state().unwrap();
    // Reproduce the state retained after failed native uninitialization without asking
    // Apple's unit to fail: native ownership and callback storage remain initialized.
    plugin.active_layout = None;
    assert!(plugin.initialized);
    assert!(
        plugin.is_active(),
        "failed teardown must stay visible to the host"
    );
    let error = plugin
        .activate_with_state(layout.clone(), 48_000.0, 128, Some(&state))
        .unwrap_err()
        .to_string();
    assert!(error.contains("requires an inactive instance"), "{error}");
    assert!(plugin.render.render_ctx.is_some());
    assert!(plugin.render.host_transport.is_some());
    assert!(plugin.activate(layout.clone(), 48_000.0, 128).is_err());
    plugin.deactivate();
    assert!(!plugin.initialized);
    plugin
        .activate_with_state(layout, 48_000.0, 128, Some(&state))
        .unwrap();
    plugin.deactivate();
}

fn measure_reload(
    info: &truce_rack_core::info::PluginInfo,
    selected: &truce_rack_core::bus::BusLayout,
    selected_only: bool,
) -> std::time::Duration {
    let start = std::time::Instant::now();
    let scanner = AuScanner::new();
    let mut plugin = if selected_only {
        scanner.load_with_layout(info, selected)
    } else {
        scanner.load(info)
    }
    .unwrap();
    plugin.activate(selected.clone(), 48_000.0, 128).unwrap();
    assert_bypass_pcm(&mut plugin, 2);
    plugin.deactivate();
    drop(plugin);
    start.elapsed()
}

#[test]
fn selected_reload_measurement() {
    let info = limiter_info();
    let owner = AuScanner::new().load(&info).unwrap();
    let selected = owner
        .supported_layouts()
        .iter()
        .find(|layout| layout.total_input_channels() == 2 && layout.total_output_channels() == 2)
        .unwrap()
        .clone();
    drop(owner);
    for _ in 0..3 {
        measure_reload(&info, &selected, false);
        measure_reload(&info, &selected, true);
    }
    let mut full = Vec::with_capacity(20);
    let mut selected_only = Vec::with_capacity(20);
    for round in 0..20 {
        // Alternate order to avoid assigning every cold invocation to one path.
        for optimized in [round % 2 == 0, round % 2 != 0] {
            let elapsed = measure_reload(&info, &selected, optimized);
            if optimized {
                selected_only.push(elapsed);
            } else {
                full.push(elapsed);
            }
        }
    }
    full.sort_unstable();
    selected_only.sort_unstable();
    eprintln!(
        "AU reload: 48kHz/128frames/2ch, debug, 3 warmups, 20 interleaved samples; full median_us={}, selected median_us={}",
        full[10].as_micros(),
        selected_only[10].as_micros()
    );
}

#[test]
fn render_geometry_failures_keep_typed_causes_without_allocating() {
    use truce_rack_core::error::{Error, ProcessFault};

    let mut plugin = load_limiter();
    let layout = plugin.supported_layouts()[0].clone();
    let channels = usize::try_from(layout.total_output_channels()).unwrap();
    plugin.activate(layout, 48_000.0, 128).unwrap();
    let input = vec![vec![0.25_f32; 129]; channels];
    let mut output = vec![vec![0.0_f32; 129]; channels];
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let ranges = [BusRange::new(0, channels)];
    let mut oversized = AudioBuffer::new(&inputs, &mut outputs, 129, &ranges, &ranges);
    let events = EventList::new();
    let mut output_events = EventList::new();
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: 128,
        transport: None,
        output_events: &mut output_events,
    };
    let allocations = allocation_counter::measure(|| {
        assert!(matches!(
            plugin.process(&mut oversized, &events, &mut context),
            Err(Error::Process {
                format: "au",
                fault: ProcessFault::InvalidBlockSize
            })
        ));
    });
    assert_eq!(allocations.count_total, 0);
    let mut no_outputs = [];
    let mut missing = AudioBuffer::new(&inputs, &mut no_outputs, 64, &ranges, &[]);
    let allocations = allocation_counter::measure(|| {
        assert!(matches!(
            plugin.process(&mut missing, &events, &mut context),
            Err(Error::Process {
                format: "au",
                fault: ProcessFault::InvalidAudioMapping
            })
        ));
    });
    assert_eq!(allocations.count_total, 0);
    plugin.deactivate();
}

#[test]
fn native_render_status_keeps_its_code_without_allocating() {
    use objc2_audio_toolbox::{AudioUnitUninitialize, kAudioUnitErr_Uninitialized};
    use truce_rack_core::error::{Error, ProcessFault};

    let mut plugin = load_limiter();
    let layout = plugin.supported_layouts()[0].clone();
    let channels = usize::try_from(layout.total_output_channels()).unwrap();
    plugin.activate(layout, 48_000.0, 128).unwrap();
    // SAFETY: the test exclusively owns a native instance with no hardware or concurrent render.
    assert_eq!(unsafe { AudioUnitUninitialize(plugin.instance) }, 0);
    // Keep the adapter's active state to exercise an AU that rejects rendering after setup.
    let input = vec![vec![0.25_f32; 64]; channels];
    let mut output = vec![vec![0.0_f32; 64]; channels];
    let inputs: Vec<&[f32]> = input.iter().map(Vec::as_slice).collect();
    let mut outputs: Vec<&mut [f32]> = output.iter_mut().map(Vec::as_mut_slice).collect();
    let ranges = [BusRange::new(0, channels)];
    let mut buffer = AudioBuffer::new(&inputs, &mut outputs, 64, &ranges, &ranges);
    let events = EventList::new();
    let mut output_events = EventList::new();
    let mut context = ProcessContext {
        sample_rate: 48_000.0,
        max_block_size: 128,
        transport: None,
        output_events: &mut output_events,
    };
    let allocations = allocation_counter::measure(|| {
        assert!(matches!(plugin.process(&mut buffer, &events, &mut context),
            Err(Error::Process { format: "au", fault: ProcessFault::NativeStatus(code) })
                if code == i64::from(kAudioUnitErr_Uninitialized)));
    });
    assert_eq!(allocations.count_total, 0);
    let ctx = plugin.render.render_ctx.as_deref().unwrap();
    assert_eq!(ctx.frames, 0);
    assert!(ctx.input_planes.iter().all(|plane| plane.is_null()));
    plugin.deactivate();
}
