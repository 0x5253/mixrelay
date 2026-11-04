//! A driver duplex endpoint captured and rendered in the same callback.
use super::{Binding, renderer::RenderSignals, tests};
use mixrelay_audio_graph::{GraphDocument, NodeConfig, NodeControl};
use mixrelay_render::{BlockProcessor, ProcessorExchange, RenderPlan, RuntimeControlUpdate};
use std::sync::{Arc, atomic::Ordering};

fn processor() -> Box<BlockProcessor> {
    let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2, "revision": 1, "nodes": [{
            "id": "virtual", "type": "virtual_endpoint",
            "config": {"id":"01010101-0101-0101-0101-010101010101","displayName":"Fixture",
                "reference":{"render":true,"capture":true},
                "render":{"channels":2,"channelLayout":"stereo"},
                "capture":{"channels":2,"channelLayout":"stereo"},
                "supportedSampleRates":[48000],"defaultSampleRate":48000}
        }], "edges": [{
            "id":"feedback","source":{"nodeId":"virtual","portId":"from_os"},
            "destination":{"nodeId":"virtual","portId":"to_os"}
        }]
    }))
    .unwrap();
    if let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[0].config {
        document.nodes[0].config = NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
    }
    let compiled = mixrelay_audio_graph::GraphCompiler::default()
        .compile(
            &document,
            mixrelay_audio_graph::CompileContext {
                format: mixrelay_audio_graph::RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 32,
                },
                limits: mixrelay_audio_graph::CompileLimits::default(),
            },
        )
        .unwrap();
    Box::new(BlockProcessor::new(Arc::new(RenderPlan::prepare(&compiled).unwrap())).unwrap())
}

#[test]
fn driver_duplex_client_reads_capture_and_writes_render_at_native_callback_boundary() {
    let mut processor = processor();
    let bindings = [
        Binding {
            id: "virtual".into(),
            input: true,
            offset: Some(0),
            channels: 2,
        },
        Binding {
            id: "virtual".into(),
            input: false,
            offset: Some(0),
            channels: 2,
        },
    ];
    let status = mixrelay_engine_core::session::AdapterRuntimeStatus::new(
        crate::macos::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::StdClock::new()),
    );
    let signals = RenderSignals::default();
    let mut renderer = tests::unclocked(&mut processor, &bindings, &status, &signals, 1);
    let exchange = ProcessorExchange::new(1);
    for (revision, enabled, muted) in [
        (2, true, false),
        (3, false, false),
        (4, true, true),
        (5, true, false),
    ] {
        exchange
            .publish_controls(
                renderer.processor.plan().controls(),
                revision,
                &[RuntimeControlUpdate {
                    node_id: "virtual".into(),
                    control: NodeControl { enabled, muted },
                    gain_db: None,
                }],
            )
            .unwrap();
        let mut input = [0.25; 64];
        // DeviceIo silences native output before invoking this Float32 render
        // hook. The renderer then adds every graph sink into that empty mix.
        let mut output = [0.0; 64];
        let source = tests::stereo_list(&mut input);
        let mut sink = tests::stereo_list(&mut output);
        // SAFETY: this unpublished fixture retains two disjoint stereo ABLs for
        // exactly 32 frames during the one synchronous renderer invocation.
        assert!(unsafe {
            renderer.render((&raw const source).cast(), (&raw mut sink).cast(), 32, None)
        });
        let expected: f32 = if enabled && !muted { 0.25 } else { 0.0 };
        assert_eq!(output.map(f32::to_bits), [expected.to_bits(); 64]);
    }
    assert_eq!(signals.progress.load(Ordering::Acquire), 4);
    assert_eq!((renderer.position, renderer.epoch), (Some(128), 1));
    assert!(!signals.invalid.load(Ordering::Acquire));
    assert!(!signals.failed.load(Ordering::Acquire));
}
