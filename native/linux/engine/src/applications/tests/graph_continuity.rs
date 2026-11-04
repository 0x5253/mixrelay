//! Native application capture through the real engine graph, with independent PCM evidence.
use super::*;
use mixrelay_audio_graph::composition::{CompositionError, SharedSource};
use mixrelay_audio_graph::{
    AUDIO_PORT_ID, ApplicationSelector, AudioDeviceOutputConfig, AudioEndpoint, ChannelMapping,
    CompileContext, CompileLimits, DeviceSelector, EdgeSpec, FROM_OS_PORT_ID, GraphCompiler,
    GraphDocument, NodeConfig, NodeId, NodeSpec, RenderFormat, TO_OS_PORT_ID,
    VirtualEndpointConfig, VirtualEndpointReference,
};
use mixrelay_audio_io::{
    AudioDeviceDescriptor, ChannelLayout, SourcePlaybackPolicy, StreamSpec, VirtualEndpointId,
    VirtualEndpointSpec,
};
#[cfg(feature = "native-lab")]
use mixrelay_engine_core::GraphResourceBackend;
use mixrelay_engine_core::catalog_execution::{
    ApplicationIdentityResolver, CatalogExecutionContext, compose_catalog,
};
#[cfg(feature = "native-lab")]
use mixrelay_test_support::{frame_sequence::SequenceContinuity, tone_continuity::ToneContinuity};
use serde_json::Value;
#[cfg(feature = "native-lab")]
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

mod documents;
#[cfg(feature = "native-lab")]
mod driver_fixture;
#[cfg(feature = "native-lab")]
mod engine;
#[cfg(feature = "native-lab")]
mod evidence;
#[cfg(feature = "native-lab")]
mod fixtures;
#[cfg(feature = "native-lab")]
mod forward;
mod observation;
mod pipewire;

#[cfg(feature = "native-lab")]
use {documents::*, engine::*, evidence::*, fixtures::*, forward::*, observation::*, pipewire::*};

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: u32 = 128;
#[cfg(feature = "native-lab")]
const WINDOW_FRAMES: usize = 480_000;
#[cfg(feature = "native-lab")]
const READ_FRAMES: usize = 128;
#[cfg(feature = "native-lab")]
const CAPTURE_NODE: &str = "graph-continuity";
#[cfg(feature = "native-lab")]
const SOURCE_APP_ID: &str = "org.mixrelay.fixture.application";
#[cfg(feature = "native-lab")]
const SOURCE_NODE_NAME: &str = "io.mixrelay.lab-application";
#[cfg(feature = "native-lab")]
const SINK_APP_ID: &str = "org.mixrelay.fixture.mute-sink";
const SINK_NODE_NAME: &str = "io.mixrelay.fixture.mute-sink";
const ENGINE_OUTPUT_NODE_NAME: &str = "io.mixrelay.engine-output.device-output";
const FORWARD_ENDPOINT_ID: &str = "lab-source-mute-X";
#[cfg(feature = "native-lab")]
const UNRELATED_APP_ID: &str = "org.mixrelay.fixture.unrelated-880";
#[cfg(feature = "native-lab")]
const COLLECTION_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(feature = "native-lab")]
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);

#[cfg(feature = "native-lab")]
fn graph_clock_rate() -> u32 {
    match std::env::var("MIXRELAY_LAB_GRAPH_RATE").as_deref() {
        Ok("44100") => 44_100,
        Ok("48000") | Err(std::env::VarError::NotPresent) => SAMPLE_RATE,
        value => panic!("unsupported native graph rate: {value:?}"),
    }
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_graph_continuity_preserves_480k_frames_through_engine() {
    let (source_path, sink_path, directory) = super::mute::fixture_paths();
    let mut evidence = GraphCaptureEvidence::new(&directory);
    let mut source = FixtureProcess::start(&source_path, &[]);
    let source_target = wait_for_owned_target(
        source.id(),
        SOURCE_APP_ID,
        SOURCE_NODE_NAME,
        "Stream/Output/Audio",
    );
    let mut independent = continuity::independent_recorder(&source_target.serial, &directory);
    let engine_identity = EngineProcessIdentity {
        process_id: std::process::id(),
        start_time: process_start_time(std::process::id()),
    };
    let process_id = engine_identity.process_id.to_string();
    let mut observer = super::mute::FixtureChild::spawn(
        &sink_path,
        &[
            "--graph-route",
            &process_id,
            &engine_identity.start_time.to_string(),
            ENGINE_OUTPUT_NODE_NAME,
        ],
        &[],
    );
    let sink_ready = observer.event("ready", None);
    let sink_node_name = verified_sink_node_name(&sink_ready, observer.id());
    let document = graph_document(source.id(), &sink_node_name);
    let result = capture_graph_window(
        &document,
        &source_target,
        &mut evidence,
        &mut observer,
        engine_identity,
    );
    let (recorder_running, recorder_status) = stop_recorder(&mut independent);
    let source_summary = continuity::finish_source(&mut source, &directory);
    let (independent_tone, independent_sequence) = continuity::independent_continuity(&directory);
    validate_graph_evidence(
        &mut evidence,
        &GraphValidation {
            directory,
            result,
            observer_id: observer.id(),
            recorder_running,
            recorder_status,
            source_summary,
            independent_tone,
            independent_sequence,
        },
    );
}

#[test]
#[cfg(feature = "native-lab")]
fn native_application_forward_lease_keeps_capture_pcm_and_restores_owned_state() {
    let (source_path, sink_path, directory) = super::mute::fixture_paths();
    let (_driver, mut source, endpoint_names) = start_forward_source(&directory, &source_path);
    let selected = wait_for_owned_target(
        source.id(),
        SOURCE_APP_ID,
        SOURCE_NODE_NAME,
        "Stream/Output/Audio",
    );
    let engine_identity = EngineProcessIdentity {
        process_id: std::process::id(),
        start_time: process_start_time(std::process::id()),
    };
    let composed = composed_source_mute_graph(source.id(), SINK_NODE_NAME);
    let output_name = composed_output_node_name(&composed.native_outputs, SINK_NODE_NAME);
    let mut observer = super::mute::FixtureChild::spawn(
        &sink_path,
        &[
            "--graph-route",
            &engine_identity.process_id.to_string(),
            &engine_identity.start_time.to_string(),
            &output_name,
        ],
        &[],
    );
    let sink_name = verified_sink_node_name(&observer.event("ready", None), observer.id());
    assert_eq!(sink_name, SINK_NODE_NAME);
    assert_eq!(composed.native_sources.len(), 1);
    assert_eq!(composed.playback_intents.len(), 1);
    let intent = &composed.playback_intents[0];
    assert_eq!(
        composed.native_sources.get(&intent.source_id),
        Some(&intent.capture_node_id)
    );
    let (evidence, stats) = with_engine_graph(
        &composed.document,
        RenderFormat {
            sample_rate: SAMPLE_RATE,
            block_frames: BLOCK_FRAMES,
        },
        &composed.playback_intents,
        |driver, (status, counters), backend, exchange, controls| {
            run_forward_session(&mut ForwardInputs {
                driver,
                status,
                counters,
                backend,
                exchange,
                controls,
                observer: &mut observer,
                directory: &directory,
                selected: &selected,
                endpoint_names: &endpoint_names,
                intent,
                engine_identity,
            })
        },
    );
    let source_summary = continuity::finish_source(&mut source, &directory);
    assert_eq!(source_summary["valid"], true, "source fixture PCM trace");
    assert_eq!(stats.sample_rate(), graph_clock_rate());
    assert!(
        stats.frames * u64::from(SAMPLE_RATE)
            >= WINDOW_FRAMES as u64 * u64::from(graph_clock_rate())
    );
    assert!(stats.cycles > 0);
    assert_eq!(evidence["observerExit"], true);
    let summary = serde_json::json!({
        "source": source_summary,
        "driver": {"cycles": stats.cycles, "frames": stats.frames,
            "rate": stats.sample_rate()},
        "evidence": evidence,
    });
    std::fs::write(
        directory.join(format!("source-mute-forward-{}.json", std::process::id())),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .expect("save actual Forward/WP ACK evidence");
    println!("native_source_mute_forward={summary}");
}
