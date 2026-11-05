//! Pacing source and head selection over application-only plans with two virtual inputs.
use super::*;
use crate::macos::external_io::plan_routes;
use mixrelay_audio_graph::{
    CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, RenderFormat,
};
use mixrelay_macos_transport::{
    application_source_transport::SourceTransport, source_control::SourceRequest,
};
use std::time::Duration;

const V0: &str = "17e59d73-42c1-4f11-a8b4-6c2d913357e0";
const V1: &str = "27e59d73-42c1-4f11-a8b4-6c2d913357e1";

/// V0 and V1, listed in that order, each recorded. `v0` is V0's node control, or `None`
/// without V0; `monitor` adds a device output that V1 also feeds, offline unless its
/// device clock runs.
fn plan(v0: Option<bool>, monitor: bool) -> RenderPlan {
    let endpoint = |id: &str, uuid: &str, enabled: bool| {
        serde_json::json!({
            "id": id, "type": "virtual_endpoint", "control": {"enabled": enabled},
            "config": {"id": uuid, "displayName": id,
                "reference": {"render": true, "capture": false},
                "render": {"channels": 2, "channelLayout": "stereo"},
                "capture": {"channels": 2, "channelLayout": "stereo"},
                "supportedSampleRates": [48000], "defaultSampleRate": 48000}
        })
    };
    let edge = |id: &str, from: &str, port: &str, to: &str| {
        serde_json::json!({"id": id, "source": {"nodeId": from, "portId": port},
            "destination": {"nodeId": to, "portId": "audio"}})
    };
    let mut nodes = vec![
        endpoint("v1", V1, true),
        serde_json::json!({"id": "r1", "type": "recorder", "config": {"channels": 2}}),
    ];
    let mut edges = vec![edge("v1-r1", "v1", "from_os", "r1")];
    if let Some(enabled) = v0 {
        nodes.insert(0, endpoint("v0", V0, enabled));
        nodes.push(serde_json::json!({"id": "r0", "type": "recorder", "config": {"channels": 2}}));
        edges.insert(0, edge("v0-r0", "v0", "from_os", "r0"));
    }
    if monitor {
        nodes.push(
            serde_json::json!({"id": "out", "type": "audio_device_output",
            "config": {"selector": {"match": "persistent_uid", "uid": "out"}, "inputChannels": 2}}),
        );
        edges.push(edge("v1-out", "v1", "from_os", "out"));
    }
    let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion": 2, "revision": 1, "nodes": nodes, "edges": edges
    }))
    .unwrap();
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(endpoint) = &node.config {
            node.config = NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
        }
    }
    let execution = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate: 48_000,
                    block_frames: 128,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    RenderPlan::prepare(&execution).unwrap()
}

fn token(plan: &RenderPlan, uuid: &str) -> u64 {
    plan.source_bindings()
        .iter()
        .find(|binding| binding.virtual_endpoint_id().unwrap().as_str() == uuid)
        .map(binding_token)
        .unwrap()
}

/// A producer that runs at the graph rate with `published` frames of credit.
fn running(published: u64) -> ApplicationClockSnapshot {
    ApplicationClockSnapshot {
        epoch: 1,
        published_frames: published,
        native_rate: 48_000,
        graph_rate: 48_000,
        running: 1,
        synchronized: 1,
        capacity_frames: 16_384,
    }
}

/// One pacing decision with each producer's clock given by token, and the routes the
/// graph then publishes for every virtual input.
fn decide(
    pacers: &mut NativePacers,
    plan: &RenderPlan,
    clocks: &[(u64, ApplicationClockSnapshot)],
) -> Vec<(u64, ConsumerRoute)> {
    let _ = next_application_pace(pacers, plan, |binding| {
        let token = binding_token(binding);
        clocks
            .iter()
            .find(|(clock, _)| *clock == token)
            .map(|(_, clock)| *clock)
    });
    let enabled = |binding: &ExternalBinding| {
        plan.controls()
            .snapshot(binding.node_id())
            .is_some_and(|(control, _)| control.enabled)
    };
    plan_routes(
        plan.source_bindings()
            .iter()
            .map(|binding| (binding, enabled(binding))),
        pacers.head(),
    )
    .collect()
}

fn selected(pacers: &NativePacers) -> Option<u64> {
    pacers.selected_application.map(|(token, _)| token)
}

#[test]
fn an_off_input_never_holds_the_graph_on_its_frozen_credit() {
    let plan = plan(Some(false), false);
    let (v0, v1) = (token(&plan, V0), token(&plan, V1));
    let mut pacers = NativePacers::default();
    // V0 still reports running with frozen credit, as an unrouted producer once did.
    let routes = decide(
        &mut pacers,
        &plan,
        &[(v0, running(4_096)), (v1, running(256))],
    );
    assert_eq!(selected(&pacers), Some(v1));
    assert_eq!(
        routes,
        [
            (v0, ConsumerRoute::Unrouted),
            (v1, ConsumerRoute::Routed { head: false })
        ]
    );
}

#[test]
fn the_published_head_is_the_source_the_pacer_awaited() {
    let plan = plan(Some(true), false);
    let (v0, v1) = (token(&plan, V0), token(&plan, V1));
    let mut pacers = NativePacers::default();
    // Nothing runs: the pacer awaits V0, and only V0 receives its retained head.
    let routes = decide(&mut pacers, &plan, &[]);
    assert_eq!((selected(&pacers), pacers.head()), (Some(v0), Some(v0)));
    assert_eq!(
        routes,
        [
            (v0, ConsumerRoute::Routed { head: true }),
            (v1, ConsumerRoute::Routed { head: false })
        ]
    );
    // V0 starts as awaited: it stays the head through its first credit.
    let routes = decide(&mut pacers, &plan, &[(v0, running(128))]);
    assert_eq!(pacers.head(), Some(v0));
    assert_eq!(routes[0], (v0, ConsumerRoute::Routed { head: true }));
}

#[test]
fn a_later_listed_source_that_starts_first_leaves_no_head_to_rewind_the_cursor() {
    let plan = plan(Some(true), false);
    let (v0, v1) = (token(&plan, V0), token(&plan, V1));
    let mut pacers = NativePacers::default();
    decide(&mut pacers, &plan, &[]);
    // V1 starts first and paces from its current credit: it was not awaited.
    let routes = decide(&mut pacers, &plan, &[(v1, running(4_096))]);
    assert_eq!((selected(&pacers), pacers.head()), (Some(v1), None));
    assert!(
        routes
            .iter()
            .all(|(_, route)| *route != ConsumerRoute::Routed { head: true })
    );
    // V0 starts later: V1 keeps pacing, and V0 joins at its live edge without a head.
    let routes = decide(
        &mut pacers,
        &plan,
        &[(v0, running(128)), (v1, running(4_224))],
    );
    assert_eq!((selected(&pacers), pacers.head()), (Some(v1), None));
    assert_eq!(routes[0], (v0, ConsumerRoute::Routed { head: false }));
    // Only when V1 stops does V0 take over.
    decide(&mut pacers, &plan, &[(v0, running(256))]);
    assert_eq!(selected(&pacers), Some(v0));
}

/// A plan with a monitor output is paced by that device only while its clock runs.
/// Offline, the graph awaits V1 and V1 keeps the recording's head; once the device
/// clock runs, nothing is awaited and no source has a head.
#[test]
fn only_a_running_device_clock_takes_the_head_from_an_application_paced_plan() {
    use crate::macos_devices::{
        MacDevices,
        callback_fixture::{CallbackTime, NativeOwner, publish},
    };
    use mixrelay_engine_core::session::AdapterRuntimeStatus;
    let plan = plan(Some(false), true);
    let v1 = token(&plan, V1);
    let resources = MacDevices::new(
        std::sync::Arc::new(AdapterRuntimeStatus::new(
            crate::macos::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        )),
        48_000,
        128,
    );
    let offline = publish(&resources, &[]);
    let mut pacers = NativePacers::default();
    let _ = next_native_pace(&mut pacers, &offline, &plan, 16_384);
    assert_eq!(pacers.head(), Some(v1));
    let output = NativeOwner::new(2, 48_000, false);
    let online = publish(&resources, &[("out", &output)]);
    let _bridge = output.serve();
    output.callback(
        &mut [0.0; 256],
        CallbackTime {
            now: 1,
            host: 2,
            sample: 0.0,
            rate_scalar: 1.0,
        },
    );
    assert!(online.hardware_clock_progress().is_some());
    let _ = next_native_pace(&mut pacers, &online, &plan, 16_384);
    assert_eq!(pacers.head(), None);
}

struct FixedClock;
impl mixrelay_time::MonotonicClock for FixedClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::ZERO)
    }
}

#[test]
fn plan_routes_unroute_dropped_sources_and_every_source_when_the_worker_ends() {
    let registry: &'static SourceRegistry = Box::leak(Box::new(SourceRegistry::new(
        Box::new(FixedClock),
        mixrelay_realtime::Timebase::current(),
    )));
    let attach = |uuid: &str| {
        let mut request = SourceRequest {
            capacity_frames: 16_384,
            channels: 2,
            transport_version: mixrelay_macos_transport::application_clock::VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request
            .source_id
            .iter_mut()
            .zip(format!("virtual:{uuid}").bytes())
        {
            *target = std::ffi::c_char::from_ne_bytes([byte]);
        }
        SourceTransport::create(registry, &request).unwrap()
    };
    let (both, only_v1) = (plan(Some(true), false), plan(None, false));
    let (v0, v1) = (attach(V0), attach(V1));
    let routed = ConsumerRoute::Routed { head: false };
    let mut worker_routes = PlanRoutes::new(Some(registry));
    worker_routes.activate(&both);
    for token in [token(&both, V0), token(&both, V1)] {
        assert!(registry.set_consumer_route(token, routed));
    }
    // The next plan no longer contains V0.
    worker_routes.activate(&only_v1);
    assert_eq!(v0.consumer_route(), ConsumerRoute::Unrouted);
    assert_eq!(v1.consumer_route(), routed);
    drop(worker_routes);
    assert_eq!(v1.consumer_route(), ConsumerRoute::Unrouted);
}
