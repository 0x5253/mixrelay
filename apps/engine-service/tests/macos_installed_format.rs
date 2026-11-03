//! Explicit installed-engine acceptance, not an isolated unit-test substitute.
//! Requires `MIXRELAY_TEST_INSTALLED_ENGINE=1` and
//! `MIXRELAY_TEST_MUTATE_SESSION=1`; omission fails before any I/O.
//! Only the existing session and own HAL nominal rate are changed. The original
//! format is restored on failure/panic, unless concurrent topology edits make
//! restoring unsafe. No graph, driver, device selection or file backup is written.
#![cfg(target_os = "macos")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a failed setup fails the test"
)]

use std::{
    collections::BTreeSet,
    error::Error,
    io::{self, Read},
    panic::AssertUnwindSafe,
    path::PathBuf,
    time::{Duration, Instant},
};

use mixrelay_audio_graph::{GraphDocument, NodeConfig};
use mixrelay_audio_io::{
    formats::{BUFFER_SIZES, SAMPLE_RATES},
    process::ProcessingState,
    session::{AudioBackendKind, AudioSessionStatus, SessionFormat, SessionReconfigureRequest},
};
use mixrelay_control_protocol::{AppStatus, GraphApplyResponse};
use mixrelay_driver_adapters::DriverClient;
use mixrelay_driver_core::DriverCommand;
use mixrelay_local_socket::{ENGINE_SOCKET, socket_directory};
use mixrelay_macos_transport::endpoint_identity::format_endpoint_id;
use mixrelay_macos_transport::endpoint_identity::parse_endpoint_id;
use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, de::DeserializeOwned};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const API: &str = "http://mixrelay-engine/api/v1";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedSession {
    version: u32,
    backend: AudioBackendKind,
    format: SessionFormat,
}

struct InstalledSession {
    client: Client,
    original_graph: GraphDocument,
    original_format: SessionFormat,
    endpoint_id: [u8; 16],
    expected_revision: u64,
    preferences: PathBuf,
    mutated: bool,
}

fn device_sample_rate(id: [u8; 16]) -> Option<u32> {
    let snapshot = DriverClient::new().ok()?.snapshot_blocking().ok()?;
    snapshot
        .settings
        .devices
        .iter()
        .find(|device| device.spec.id.as_str() == format_endpoint_id(id))?
        .observed_sample_rate
}

fn set_device_sample_rate(id: [u8; 16], sample_rate: u32) -> bool {
    let result = (|| -> Result<()> {
        let client = DriverClient::new()?;
        let snapshot = client.snapshot_blocking()?;
        let mut spec = snapshot
            .settings
            .devices
            .iter()
            .find(|device| device.spec.id.as_str() == format_endpoint_id(id))
            .ok_or("Driver device is absent")?
            .spec
            .clone();
        spec.default_sample_rate = sample_rate;
        let readback = client.mutate_blocking(&DriverCommand::Configure {
            expected_revision: snapshot.revision,
            spec,
        })?;
        require(
            readback.settings.devices.iter().any(|device| {
                device.spec.id.as_str() == format_endpoint_id(id)
                    && device.observed_sample_rate == Some(sample_rate)
            }),
            "Driver nominal-rate readback differs",
        )?;
        Ok(())
    })();
    result.is_ok()
}

fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message.into()).into())
    }
}

fn topology(mut graph: GraphDocument) -> GraphDocument {
    graph.revision = 0;
    for node in &mut graph.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &mut node.config {
            // Session format transactions legitimately refresh these metadata
            // fields. Channels, IDs, labels, processors and all edges stay exact.
            config.spec.default_sample_rate = 0;
            config.spec.supported_sample_rates.clear();
        }
    }
    graph
}

fn referenced_endpoint_ids(graph: &GraphDocument) -> Result<BTreeSet<[u8; 16]>> {
    let mut ids = BTreeSet::new();
    for node in &graph.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &node.config {
            let text = config.spec.id.as_str();
            let id = parse_endpoint_id(text).ok_or_else(|| {
                io::Error::other(format!(
                    "referenced endpoint must have a nonzero canonical UUID: {text}"
                ))
            })?;
            ids.insert(id);
        }
    }
    require(
        !ids.is_empty(),
        "installed test requires a referenced virtual endpoint UUID",
    )?;
    Ok(ids)
}

fn single_endpoint_id(graph: &GraphDocument) -> Result<[u8; 16]> {
    let ids = referenced_endpoint_ids(graph)?;
    require(
        ids.len() == 1,
        "format acceptance requires one distinct endpoint identity",
    )?;
    ids.into_iter()
        .next()
        .ok_or_else(|| io::Error::other("endpoint identity disappeared").into())
}

fn referenced_native_rates(graph: &GraphDocument) -> Result<Vec<([u8; 16], u32)>> {
    referenced_endpoint_ids(graph)?
        .into_iter()
        .map(|id| {
            let rate = device_sample_rate(id).ok_or("referenced HAL device must be present")?;
            Ok((id, rate))
        })
        .collect()
}

fn preferences_path() -> Result<PathBuf> {
    let directory = if let Some(path) = std::env::var_os("MIXRELAY_CONFIG_DIR") {
        PathBuf::from(path)
    } else {
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?)
            .join("Library/Application Support/MixRelay")
    };
    require(
        directory.is_absolute(),
        "preferences directory must be absolute",
    )?;
    Ok(directory.join("audio-session.json"))
}

impl InstalledSession {
    fn connect() -> Result<Self> {
        let client = Client::builder()
            .unix_socket(socket_directory()?.join(ENGINE_SOCKET))
            .timeout(Duration::from_secs(12))
            .build()?;
        let initial: AppStatus = client
            .get(format!("{API}/status"))
            .send()?
            .error_for_status()?
            .json()?;
        let initial = initial
            .audio_session
            .ok_or("installed engine has no audio session")?;
        let graph: GraphDocument = client
            .get(format!("{API}/graph"))
            .send()?
            .error_for_status()?
            .json()?;
        require(
            initial.capabilities.backend == AudioBackendKind::CoreAudio,
            "expected CoreAudio engine",
        )?;
        require(
            initial.capabilities.supported_sample_rates == SAMPLE_RATES,
            "installed HAL must expose all six product rates",
        )?;
        require(
            initial.capabilities.supported_block_frames == BUFFER_SIZES,
            "engine must expose all seven buffer sizes",
        )?;
        require(
            initial.issue.is_none(),
            format!("initial session issue: {:?}", initial.issue),
        )?;
        // Select the graph's actual referenced identity, never a singleton UID.
        // Multiple references to that identity share one native device.
        let endpoint_id = single_endpoint_id(&graph)?;
        require(
            device_sample_rate(endpoint_id) == Some(initial.format.sample_rate),
            "initial own-HAL and engine sample rates differ",
        )?;
        let session = Self {
            client,
            expected_revision: graph.revision,
            original_graph: topology(graph),
            original_format: initial.format,
            endpoint_id,
            preferences: preferences_path()?,
            mutated: false,
        };
        require(
            session.saved_format()? == initial.format,
            "start only from matching saved/active settings",
        )?;
        session.unchanged_graph()?;
        Ok(session)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(self
            .client
            .get(format!("{API}{path}"))
            .send()?
            .error_for_status()?
            .json()?)
    }

    fn status(&self) -> Result<AudioSessionStatus> {
        self.get::<AppStatus>("/status")?
            .audio_session
            .ok_or_else(|| io::Error::other("audio session disappeared").into())
    }

    fn saved_format(&self) -> Result<SessionFormat> {
        let mut bytes = Vec::with_capacity(4097);
        std::fs::File::open(&self.preferences)?
            .take(4097)
            .read_to_end(&mut bytes)?;
        require(bytes.len() <= 4096, "saved session exceeds size limit")?;
        let saved: SavedSession = serde_json::from_slice(&bytes)?;
        require(
            saved.version == 1 && saved.backend == AudioBackendKind::CoreAudio,
            "unexpected persisted session version/backend",
        )?;
        Ok(saved.format)
    }

    fn unchanged_graph(&self) -> Result<GraphDocument> {
        let graph: GraphDocument = self.get("/graph")?;
        require(
            topology(graph.clone()) == self.original_graph,
            "concurrent graph/topology edit: refusing further mutation or automatic restore",
        )?;
        Ok(graph)
    }

    fn settled(&mut self, expected: SessionFormat, label: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let graph = self.unchanged_graph()?;
            let status = self.status()?;
            // Buffer size is the engine's processing quantum, not another
            // CoreAudio client's BufferFrameSize. Read back own HAL rate only.
            let native_rate = device_sample_rate(self.endpoint_id);
            if status.format == expected
                && status.observed_device_sample_rate == Some(expected.sample_rate)
                && status.issue.is_none()
                && native_rate == Some(expected.sample_rate)
                && status.processing_state == ProcessingState::Running
                && self.saved_format()? == expected
            {
                self.expected_revision = graph.revision;
                println!(
                    "{label}: {} Hz / {} samples; native={native_rate:?}; persisted; revision={}",
                    expected.sample_rate, expected.block_frames, graph.revision
                );
                return Ok(());
            }
            require(
                Instant::now() < deadline,
                format!(
                    "{label} did not settle at {expected:?}; native={native_rate:?}; session={status:?}"
                ),
            )?;
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn apply(&mut self, format: SessionFormat, label: &str) -> Result<()> {
        let graph = self.unchanged_graph()?;
        require(
            graph.revision == self.expected_revision,
            "concurrent graph revision before format request",
        )?;
        // Set before sending: a timeout can occur after the server committed.
        self.mutated = true;
        let response = self
            .client
            .put(format!("{API}/audio/session"))
            .json(&SessionReconfigureRequest {
                expected_revision: graph.revision,
                format,
            })
            .send()?;
        let status = response.status();
        let body = response.text()?;
        require(
            status == StatusCode::ACCEPTED,
            format!("format request {format:?}: HTTP {status}: {body}"),
        )?;
        let accepted: GraphApplyResponse = serde_json::from_str(&body)?;
        require(
            accepted.persistence_error.is_none(),
            format!("persistence failure: {:?}", accepted.persistence_error),
        )?;
        self.settled(format, label)
    }

    fn exercise(&mut self) -> Result<()> {
        for &sample_rate in SAMPLE_RATES {
            for &block_frames in BUFFER_SIZES {
                self.apply(
                    SessionFormat {
                        sample_rate,
                        block_frames,
                    },
                    "app → HAL",
                )?;
            }
        }
        let block_frames = self.status()?.format.block_frames;
        for &sample_rate in SAMPLE_RATES {
            let graph = self.unchanged_graph()?;
            require(
                graph.revision == self.expected_revision,
                "concurrent revision before external rate change",
            )?;
            self.mutated = true;
            // Direct device setting models another CoreAudio application's rate
            // change. No graph/session API request triggers this adoption.
            require(
                set_device_sample_rate(self.endpoint_id, sample_rate),
                format!("own HAL rejected external rate {sample_rate}"),
            )?;
            self.settled(
                SessionFormat {
                    sample_rate,
                    block_frames,
                },
                "HAL → app",
            )?;
        }
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        if !self.mutated {
            return Ok(());
        }
        // Never submit a saved graph to roll back. Restore only session format,
        // guarded by the current graph revision and unchanged full topology.
        self.expected_revision = self.unchanged_graph()?.revision;
        self.apply(self.original_format, "restore original")
    }
}

#[test]
fn installed_engine_accepts_every_format_and_follows_external_own_hal_rates() {
    assert_eq!(
        std::env::var("MIXRELAY_TEST_MUTATE_SESSION").as_deref(),
        Ok("1"),
        "format mutation needs explicit MIXRELAY_TEST_MUTATE_SESSION=1"
    );
    assert_eq!(
        std::env::var("MIXRELAY_TEST_INSTALLED_ENGINE").as_deref(),
        Ok("1"),
        "explicit authorization required: MIXRELAY_TEST_INSTALLED_ENGINE=1; no installed state was touched"
    );
    let mut session = InstalledSession::connect().expect("installed engine preflight");
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| session.exercise()));
    let restored = session.restore();
    assert!(
        restored.is_ok(),
        "original format restoration failed: {restored:?}; test result: {result:?}"
    );
    match result {
        Ok(result) => result.expect("installed format acceptance"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn read_installed<T: DeserializeOwned>(client: &Client, path: &str) -> T {
    client
        .get(format!("{API}/{path}"))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap()
}

/// Apply only a buffer change while proving that the original route still exists.
fn apply_existing_buffer(
    client: &Client,
    original_graph: &GraphDocument,
    original: SessionFormat,
    block_frames: u32,
) {
    let current = read_installed::<GraphDocument>(client, "graph");
    assert_eq!(
        topology(current.clone()),
        topology(original_graph.clone()),
        "concurrent topology change"
    );
    let response = client
        .put(format!("{API}/audio/session"))
        .json(&SessionReconfigureRequest {
            expected_revision: current.revision,
            format: SessionFormat {
                sample_rate: original.sample_rate,
                block_frames,
            },
        })
        .send()
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "{}",
        response.text().unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let session = read_installed::<AppStatus>(client, "status")
            .audio_session
            .unwrap();
        if session.format
            == (SessionFormat {
                sample_rate: original.sample_rate,
                block_frames,
            })
            && session.processing_state == ProcessingState::Running
            && session.issue.is_none()
        {
            break;
        }
        assert!(Instant::now() < deadline, "{session:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Hardware routing follows its real device clock; referenced virtual devices
/// must not be retuned by buffer changes. This test measures the existing route.
#[test]
fn installed_hardware_route_has_no_starvations_at_32_and_256() {
    assert_eq!(
        std::env::var("MIXRELAY_TEST_MUTATE_SESSION").as_deref(),
        Ok("1"),
        "buffer mutation needs explicit MIXRELAY_TEST_MUTATE_SESSION=1"
    );
    assert_eq!(
        std::env::var("MIXRELAY_TEST_INSTALLED_ENGINE").as_deref(),
        Ok("1")
    );
    let client = Client::builder()
        .unix_socket(socket_directory().unwrap().join(ENGINE_SOCKET))
        .timeout(Duration::from_secs(12))
        .build()
        .unwrap();
    let status = || read_installed::<AppStatus>(&client, "status");
    let graph = || read_installed::<GraphDocument>(&client, "graph");
    let original = status().audio_session.unwrap().format;
    let original_graph = graph();
    assert!(
        original_graph
            .nodes
            .iter()
            .any(|node| matches!(node.config, NodeConfig::AudioDeviceInput(_)))
    );
    assert!(
        original_graph
            .nodes
            .iter()
            .any(|node| matches!(node.config, NodeConfig::AudioDeviceOutput(_)))
    );
    assert!(!original_graph.edges.is_empty());
    let hal_rates = referenced_native_rates(&original_graph)
        .expect("hardware acceptance requires present referenced virtual endpoint identities");
    let apply =
        |block_frames| apply_existing_buffer(&client, &original_graph, original, block_frames);
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut clean = true;
        for block_frames in [32, 256] {
            apply(block_frames);
            std::thread::sleep(Duration::from_secs(1));
            let before = status();
            let start = Instant::now();
            std::thread::sleep(Duration::from_secs(5));
            let after = status();
            let input = after.underruns.saturating_sub(before.underruns);
            let output = after.overruns.saturating_sub(before.overruns);
            let discontinuities = after
                .audio_session
                .unwrap()
                .discontinuities
                .saturating_sub(before.audio_session.unwrap().discontinuities);
            println!(
                "continuity {}/{block_frames}: duration_ms={} input_starvations={input} output_overflows={output} discontinuities={discontinuities}",
                original.sample_rate,
                start.elapsed().as_millis()
            );
            clean &= input == 0 && output == 0 && discontinuities == 0;
        }
        for (id, rate) in &hal_rates {
            assert_eq!(
                device_sample_rate(*id),
                Some(*rate),
                "referenced HAL device was retuned or removed"
            );
        }
        assert!(
            clean,
            "live hardware route starved, overflowed or lost its native clock"
        );
    }));
    apply(original.block_frames);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn installed_format_target_requires_one_actual_uuid_without_default_fallback() {
    use mixrelay_audio_graph::NodeSpec;
    use mixrelay_audio_io::{ChannelLayout, StreamSpec, VirtualEndpointSpec};

    let node = |name: &str, identity: &str| NodeSpec {
        control: mixrelay_audio_graph::NodeControl::default(),
        id: name.into(),
        label: None,
        config: NodeConfig::VirtualEndpoint(
            VirtualEndpointSpec {
                id: identity.into(),
                display_name: "Identity fixture".into(),
                render: Some(StreamSpec {
                    channels: 2,
                    channel_layout: ChannelLayout::Stereo,
                }),
                capture: None,
                supported_sample_rates: vec![48_000],
                default_sample_rate: 48_000,
            }
            .into(),
        ),
    };
    let identity = "12345678-1234-4234-8234-123456789abc";
    let graph = GraphDocument::new(
        1,
        vec![node("first", identity), node("reference", identity)],
        vec![],
    );
    assert_eq!(
        single_endpoint_id(&graph).unwrap(),
        parse_endpoint_id(identity).unwrap()
    );
    assert!(single_endpoint_id(&GraphDocument::new(1, vec![], vec![])).is_err());
    for invalid in [
        "default",
        "00000000-0000-0000-0000-000000000000",
        "12345678-1234-4234-8234-123456789ABC",
    ] {
        assert!(
            single_endpoint_id(&GraphDocument::new(
                1,
                vec![node("invalid", invalid)],
                vec![]
            ))
            .is_err()
        );
    }
    let mut multiple = graph;
    multiple
        .nodes
        .push(node("another", "87654321-4321-4321-8321-cba987654321"));
    assert!(single_endpoint_id(&multiple).is_err());
    assert_eq!(referenced_endpoint_ids(&multiple).unwrap().len(), 2);
}
