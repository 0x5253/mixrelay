//! Callback rendering through real ABLs, without `CoreAudio`.
use super::{
    Binding,
    buffers::Buffers,
    callback_io::CallbackIo,
    renderer::{RenderSignals, Renderer},
};
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GraphCompiler, GraphDocument, RenderFormat,
};
use mixrelay_audio_io::{
    process::{ClockDomain, InputActivity, ProcessCycle, TransferOutcome},
    session::ClockAuthority,
};
use mixrelay_engine_core::{RuntimeStatusBackend, session::AdapterRuntimeStatus};
use mixrelay_macos_transport::{
    audio_buffer::AudioBuffer,
    device_io::{IoTimestamp, IoTiming},
};
use mixrelay_render::{BlockProcessor, ExternalIo, RenderPlan};
use std::sync::{Arc, atomic::Ordering};

#[repr(C)]
pub(super) struct AudioBufferList {
    count: u32,
    pub(super) first: AudioBuffer,
}

/// A renderer on clock 7 without a device clock or graph activation.
pub(super) fn unclocked<'a>(
    processor: &'a mut Box<BlockProcessor>,
    bindings: &'a [Binding],
    status: &'a AdapterRuntimeStatus,
    signals: &'a RenderSignals,
    epoch: u64,
) -> Renderer<'a> {
    Renderer {
        timebase: mixrelay_realtime::Timebase::current(),
        processor,
        activation: None,
        bindings,
        clock_id: 7,
        epoch,
        native_clock: None,
        status,
        position: None,
        signals,
    }
}

pub(super) fn status() -> AdapterRuntimeStatus {
    AdapterRuntimeStatus::new(
        crate::macos::adapter_profile(),
        Arc::new(mixrelay_time::ManualClock::default()),
    )
}

pub(super) fn stereo_bindings() -> [Binding; 2] {
    [
        Binding {
            id: "input".into(),
            input: true,
            offset: Some(0),
            channels: 2,
        },
        Binding {
            id: "output".into(),
            input: false,
            offset: Some(0),
            channels: 2,
        },
    ]
}

pub(super) fn stereo_list(pcm: &mut [f32]) -> AudioBufferList {
    AudioBufferList {
        count: 1,
        first: AudioBuffer {
            channels: 2,
            byte_size: u32::try_from(std::mem::size_of_val(pcm)).unwrap(),
            data: pcm.as_mut_ptr(),
        },
    }
}

pub(super) fn processor_at(block_frames: u32, sample_rate: u32) -> Box<BlockProcessor> {
    let document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
            {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
        ],"edges":[{"id":"wire","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}}]
    })).unwrap();
    let execution = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate,
                    block_frames,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

fn offline_branch_processor() -> Box<BlockProcessor> {
    let document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
            {"id":"missing-input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
            {"id":"output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}},
            {"id":"missing-output","type":"audio_device_output","config":{"selector":{"match":"default_output"},"inputChannels":2}}
        ],"edges":[
            {"id":"live","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}},
            {"id":"silent","source":{"nodeId":"missing-input","portId":"audio"},"destination":{"nodeId":"output","portId":"audio"}},
            {"id":"discard","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"missing-output","portId":"audio"}}
        ]
    })).unwrap();
    let execution = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&execution).unwrap())).unwrap())
}

#[test]
fn offline_transfers_are_inactive_silence_and_discard_with_complete_frame_counts() {
    let processor = processor_at(32, 48_000);
    let mut bindings = stereo_bindings();
    for binding in &mut bindings {
        binding.offset = None;
    }
    let mut io = CallbackIo {
        buffers: Buffers::empty(),
        bindings: &bindings,
        offset: 0,
        bridged: None,
    };
    assert!(bindings.iter().all(|binding| io.buffers.covers(binding)));
    let cycle = ProcessCycle {
        clock: ClockDomain {
            authority: ClockAuthority::Device,
            id: 7,
            epoch: 3,
        },
        sample_rate: 48_000,
        frames: 32,
        first_frame: 0,
        monotonic_ns: None,
        discontinuity: true,
    };
    // The engine hands every source a zeroed buffer; offline input leaves it silent.
    let mut pcm = [0.0; 64];
    let input = io.read_source(&processor.plan().source_bindings()[0], &mut pcm, &cycle);
    assert_eq!(
        input,
        TransferOutcome {
            activity: InputActivity::Inactive,
            ..TransferOutcome::complete(32)
        }
    );
    assert_eq!(pcm.map(f32::to_bits), [0.0_f32; 64].map(f32::to_bits));
    assert_eq!(
        io.write_sink(&processor.plan().sink_bindings()[0], &[0.5; 64], &cycle),
        TransferOutcome::complete(32)
    );
}

#[test]
fn native_inputs_keep_their_own_clock_instead_of_a_registry_epoch() {
    let processor = processor_at(32, 48_000);
    let bindings = stereo_bindings();
    let io = CallbackIo {
        buffers: Buffers::empty(),
        bindings: &bindings,
        offset: 0,
        bridged: None,
    };
    assert_eq!(
        io.source_epoch(&processor.plan().source_bindings()[0]),
        None
    );
}

#[test]
fn missing_device_branches_do_not_interrupt_live_callback_pcm_or_create_xruns() {
    let mut processor = offline_branch_processor();
    let mut bindings = Vec::from(stereo_bindings());
    for (id, input) in [("missing-input", true), ("missing-output", false)] {
        bindings.push(Binding {
            id: id.into(),
            input,
            offset: None,
            channels: 2,
        });
    }
    let status = status();
    let signals = RenderSignals::default();
    let mut renderer = unclocked(&mut processor, &bindings, &status, &signals, 3);
    exercise_live_callback(&mut renderer);
    assert_eq!(signals.progress.load(Ordering::Acquire), 10);
    assert!(!signals.invalid.load(Ordering::Acquire));
    assert!(!signals.failed.load(Ordering::Acquire));
    let measured = status.status(RenderFormat {
        sample_rate: 48_000,
        block_frames: 32,
    });
    assert_eq!((measured.underruns, measured.overruns), (0, 0));
}

fn exercise_live_callback(renderer: &mut Renderer<'_>) {
    let mut input = [0.25; 98];
    let mut output = [0.0; 98];
    let input_list = stereo_list(&mut input);
    let mut output_list = stereo_list(&mut output);
    for _ in 0..10 {
        output.fill(0.0);
        // SAFETY: both ABLs own 49 stereo frames for this synchronous call;
        // the two offline maps have no native storage to dereference.
        assert!(unsafe {
            renderer.render(
                (&raw const input_list).cast(),
                (&raw mut output_list).cast(),
                49,
                None,
            )
        });
        assert_eq!(output.map(f32::to_bits), input.map(f32::to_bits));
    }
}

#[test]
fn switching_between_fallback_and_native_timestamps_never_stops_rendering() {
    let mut processor = processor_at(32, 48000);
    let status = status();
    let signals = RenderSignals::default();
    let bindings = stereo_bindings();
    let mut renderer = unclocked(&mut processor, &bindings, &status, &signals, 3);
    let mut input = vec![0.25; 64];
    let mut output = vec![0.0; 64];
    let input_list = stereo_list(&mut input);
    let mut output_list = stereo_list(&mut output);
    let native = |sample_time: f64| IoTiming {
        now: IoTimestamp::default(),
        input: IoTimestamp {
            sample_time,
            host_time: 0,
            rate_scalar: 1.0,
            flags: 0,
        },
        output: IoTimestamp {
            sample_time,
            host_time: 1_000,
            rate_scalar: 1.0,
            flags: 3,
        },
    };
    // Missing, native far ahead, continuing, missing again, then a device skip.
    let timings = [
        None,
        Some(native(96_000.0)),
        Some(native(96_032.0)),
        None,
        Some(native(200_000.0)),
    ];
    let expected = [32, 96_032, 96_064, 96_096, 200_032];
    for (timing, position) in timings.iter().zip(expected) {
        assert!(unsafe {
            renderer.render(
                (&raw const input_list).cast(),
                (&raw mut output_list).cast(),
                32,
                timing.as_ref(),
            )
        });
        assert_eq!(renderer.position, Some(position));
    }
    assert!(!signals.failed.load(Ordering::Acquire));
    assert_eq!(signals.progress.load(Ordering::Acquire), 5);
}

#[test]
fn synchronous_callback_preserves_every_sample_at_all_product_buffers() {
    for &block_frames in mixrelay_audio_io::formats::BUFFER_SIZES {
        let mut processor = processor_at(block_frames, 48000);
        let status = status();
        let signals = RenderSignals::default();
        let bindings = stereo_bindings();
        let mut renderer = unclocked(&mut processor, &bindings, &status, &signals, 3);
        // Native callback larger than the DSP quantum proves split offsets,
        // including non-divisible final blocks. No queued next-call output.
        let frames = (block_frames + 17).min(4096);
        let mut input = vec![0.0; frames as usize * 2];
        let mut output = vec![0.0; input.len()];
        let input_list = stereo_list(&mut input);
        let mut output_list = stereo_list(&mut output);
        for cycle in 0..100 {
            for (index, sample) in input.iter_mut().enumerate() {
                *sample = f32::from(
                    u16::try_from((cycle * input_list.first.byte_size as usize + index) % 8192)
                        .unwrap(),
                ) / 8192.0;
            }
            output.fill(0.0);
            assert!(unsafe {
                renderer.render(
                    (&raw const input_list).cast(),
                    (&raw mut output_list).cast(),
                    frames,
                    None,
                )
            });
            assert_eq!(input, output, "buffer={block_frames} cycle={cycle}");
        }
        assert_eq!(signals.progress.load(Ordering::Acquire), 100);
        assert_eq!(renderer.position, Some(u64::from(frames) * 100));
        let measured = status.status(RenderFormat {
            sample_rate: 48000,
            block_frames,
        });
        assert_eq!(measured.underruns, 0);
        assert_eq!(measured.overruns, 0);
    }
}

#[test]
fn disabled_unused_stream_keeps_offsets_but_selected_missing_pcm_is_rejected() {
    #[repr(C)]
    struct List {
        count: u32,
        buffers: [AudioBuffer; 2],
    }
    let mut input = [0.25_f32; 64];
    let list = List {
        count: 2,
        buffers: [
            AudioBuffer {
                channels: 3,
                byte_size: 0,
                data: std::ptr::null_mut(),
            },
            AudioBuffer {
                channels: 2,
                byte_size: 256,
                data: input.as_mut_ptr(),
            },
        ],
    };
    let map = Binding {
        id: "input".into(),
        input: true,
        offset: Some(3),
        channels: 2,
    };
    let mut destination = [0.0; 64];
    // SAFETY: sized list and live PCM are retained across this operation;
    // inactive metadata has no PCM and destination is separate scratch.
    unsafe {
        Buffers::with_native((&raw const list).cast(), std::ptr::null_mut(), 32, |view| {
            assert!(view.covers(&map));
            assert!(!view.covers(&Binding {
                id: "missing".into(),
                input: true,
                offset: Some(2),
                channels: 2,
            }));
            view.transfer(&map, 0, &mut destination);
        })
    }
    .unwrap();
    assert_eq!(destination.map(f32::to_bits), input.map(f32::to_bits));
    // SAFETY: metadata is live; the advertised PCM byte count is deliberately
    // inconsistent with 64 frames and rejected before a PCM reference exists.
    assert!(
        unsafe { Buffers::with_native((&raw const list).cast(), std::ptr::null_mut(), 64, |_| ()) }
            .is_none()
    );
}
