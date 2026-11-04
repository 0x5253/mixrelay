use super::*;
use mixrelay_audio_graph::ChannelRoute;
use mixrelay_audio_graph::GraphDocument;
use std::{
    thread,
    time::{Duration, Instant},
};

fn independent_graph(id: &str) -> GraphDocument {
    let mut document = input_graph();
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &mut node.config {
            config.spec = device_spec(id);
        }
    }
    document
}

fn set_width_and_name(session: &Session, channels: u16, name: &str) {
    let stream = StreamSpec {
        channels,
        channel_layout: ChannelLayout::Discrete,
    };
    let mut spec = device_spec(A);
    spec.render = Some(stream);
    spec.capture = Some(stream);
    spec.display_name = name.into();
    configure_driver(spec);
    let mut document = session.controller.graph();
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &mut node.config
            && config.id.as_str() == A
        {
            let layout = match channels {
                1 => ChannelLayout::Mono,
                2 => ChannelLayout::Stereo,
                _ => ChannelLayout::Discrete,
            };
            let stream = StreamSpec {
                channels,
                channel_layout: layout,
            };
            config.spec.render = Some(stream);
            config.spec.capture = Some(stream);
            config.spec.display_name = name.into();
            node.label = Some(name.into());
        }
    }
    document.edges[0].channel_mapping = ChannelMapping::Explicit {
        routes: (0..channels.min(32))
            .map(|channel| ChannelRoute {
                source_channel: channel,
                destination_channel: channel,
            })
            .collect(),
    };
    session.apply(document);
}

fn stream_for(id: &str, side: &str, role: StreamRole<'_>, channels: u16) -> NativeStream {
    let stream = NativeStream::open_format(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &format!("lab.settings-{side}"),
        "Owned endpoint settings observer",
        role,
        channels,
        crate::runtime::native_format(render_format()),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !stream.ready() || !stream.connected() {
        assert!(Instant::now() < deadline, "{id} {side} did not connect");
        thread::sleep(Duration::from_millis(5));
    }
    stream
}

fn phase_checkpoint(channels: u16, phase: &str) {
    if channels != 64 {
        return;
    }
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let rss = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .unwrap();
    let high_water = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap();
    eprintln!(
        "settings_checkpoint width={channels} phase={phase} pid={} {rss} {high_water}",
        std::process::id()
    );
}

fn write_render_pcm(stream: &mut NativeStream, channels: u16) {
    let pcm = vec![0.125; 128 * usize::from(channels)];
    let deadline = Instant::now() + Duration::from_secs(3);
    phase_checkpoint(channels, "render_write_before");
    while stream.write(&pcm) == 0 {
        assert!(
            Instant::now() < deadline,
            "OS render PCM did not reach endpoint"
        );
        thread::sleep(Duration::from_millis(5));
    }
    phase_checkpoint(channels, "render_write_after");
}

fn assert_capture_activity(stream: &mut NativeStream, channels: usize) {
    let mut pcm = vec![0.0; 128 * channels];
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let frames = stream.read(&mut pcm) as usize;
        if pcm[..frames * channels]
            .chunks_exact(channels)
            .any(|frame| frame[0].abs() > 0.0001)
        {
            return;
        }
        assert!(Instant::now() < deadline, "capture PCM remained silent");
        thread::sleep(Duration::from_millis(5));
    }
}

fn assert_native_settings(channels: u16, name: &str) {
    let output = std::process::Command::new("pw-dump").output().unwrap();
    assert!(output.status.success());
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let names = native_names(&device_spec(A));
    let nodes: Vec<_> = snapshot
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| {
            names.iter().any(|name| {
                node.pointer("/info/props/node.name")
                    .and_then(serde_json::Value::as_str)
                    == Some(name.as_str())
            })
        })
        .collect();
    assert_eq!(nodes.len(), 2);
    for node in nodes {
        assert_eq!(node["info"]["props"]["node.description"], name);
        assert_eq!(
            node.pointer("/info/props/audio.channels")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(channels))
        );
        let formats = node["info"]["params"]["Format"].as_array().unwrap();
        assert_eq!(formats.len(), 1);
        assert_eq!(formats[0]["channels"], u64::from(channels));
        assert_eq!(formats[0]["rate"], 48_000);
    }
    let fixture =
        std::path::PathBuf::from(std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap());
    let path = fixture.with_file_name(format!("settings-width-{channels}.json"));
    std::fs::write(path, output.stdout).unwrap();
}

#[test]
fn native_endpoint_name_width_restore_keeps_independent_pcm() {
    let _driver_devices = DriverFixture::new();
    let session = Session::new();
    let _fixture = fixture();
    let graph_a = session.controller.graph_catalog().unwrap().active_id;
    session.apply(input_graph());
    let graph_b = session.catalog(GraphCatalogAction::Create {
        name: "Independent reader".into(),
    });
    let mut b_document = independent_graph(B);
    b_document.nodes.push(reference(A, "shared-a"));
    session.apply(b_document);
    let before = inventory();
    assert_eq!(before.len(), 4);
    let b_names = native_names(&device_spec(B));
    let mut b_capture = reader(B);
    assert_pcm(&mut b_capture);

    session.catalog(GraphCatalogAction::Select {
        id: graph_a.clone(),
    });
    for (channels, name) in [
        (1, "QA mono bus"),
        (32, "QA wide bus"),
        (64, "QA maximum bus"),
        (2, "QA restored stereo bus"),
    ] {
        set_width_and_name(&session, channels, name);
        let native = inventory();
        for b_name in &b_names {
            assert_eq!(native.get(b_name), before.get(b_name));
        }
        let a_names = native_names(&device_spec(A));
        let render = native[&a_names[0]].clone();
        let capture = native[&a_names[1]].clone();
        phase_checkpoint(channels, "render_stream_before");
        let mut os_playback = stream_for(
            A,
            "render",
            StreamRole::AutomaticDeviceOutput(&render),
            channels,
        );
        phase_checkpoint(channels, "render_stream_after");
        phase_checkpoint(channels, "capture_stream_before");
        let mut os_capture = stream_for(A, "capture", StreamRole::DeviceInput(&capture), channels);
        phase_checkpoint(channels, "capture_stream_after");
        write_render_pcm(&mut os_playback, channels);
        phase_checkpoint(channels, "capture_read_before");
        assert_capture_activity(&mut os_capture, usize::from(channels));
        phase_checkpoint(channels, "capture_read_after");
        phase_checkpoint(channels, "native_snapshot_before");
        assert_native_settings(channels, name);
        phase_checkpoint(channels, "native_snapshot_after");
        assert_pcm(&mut b_capture);
        println!("width={channels}; name={name}; independent_pcm=passed");
    }

    session.catalog(GraphCatalogAction::Delete { id: graph_a });
    assert_eq!(
        inventory().len(),
        4,
        "shared A reference must retain its owner"
    );
    let mut survivor = session.controller.graph();
    survivor.nodes.retain(|node| node.id.as_str() != "shared-a");
    session.apply(survivor);
    assert_eq!(
        inventory().len(),
        2,
        "final A reference must remove its native owner"
    );
    assert_pcm(&mut b_capture);
    drop(b_capture);
    session.catalog(GraphCatalogAction::Delete { id: graph_b });
    assert!(inventory().is_empty());
    println!("render_capture_widths=1,32,64,2; independent_pcm=passed; final_owned_nodes=0");
}
