use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

fn assert_native_name(id: &str, name: &str) {
    let names = native_names(&device_spec(id));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let targets = mixrelay_linux_audio_io::targets().unwrap();
        if names.iter().all(|native| {
            targets.iter().any(|target| {
                target.node_name.as_ref() == Some(native) && target.display_name == name
            })
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native description did not become {name}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn rename_document(session: &Session, name: &str) {
    let mut spec = device_spec(A);
    spec.display_name = name.into();
    configure_driver(spec);
    let mut document = session.controller.graph();
    for node in &mut document.nodes {
        if let NodeConfig::VirtualEndpoint(config) = &mut node.config
            && config.id.as_str() == A
        {
            config.display_name = name.into();
        }
    }
    session.apply(document);
}

fn assert_catalog_name(session: &Session, name: &str) {
    let catalog = session.controller.graph_catalog().unwrap();
    for graph in &catalog.graphs {
        let document = session
            .controller
            .graph_catalog_document(&graph.id)
            .unwrap();
        for node in &document.nodes {
            if let NodeConfig::VirtualEndpoint(config) = &node.config
                && config.id.as_str() == A
            {
                assert_eq!(config.display_name, name);
                assert_eq!(node.label.as_deref(), Some(name));
            }
        }
    }
}

#[test]
fn native_rename_preserves_device_serials_and_shared_pcm() {
    let _driver_devices = DriverFixture::new();
    let session = Session::new();
    let _fixture = fixture();
    let graph_a = session.controller.graph_catalog().unwrap().active_id;
    session.apply(input_graph());
    let graph_b = session.catalog(GraphCatalogAction::Create {
        name: "Reader".into(),
    });
    session.apply(forwarding_graph());
    let mut direct = reader(A);
    let mut forwarded = reader(B);
    assert_pcm(&mut direct);
    assert_pcm(&mut forwarded);
    let before = inventory();
    session.catalog(GraphCatalogAction::Select { id: graph_a });
    rename_document(&session, "Renamed shared bus");
    let after = inventory();
    let reopened = before
        .iter()
        .filter(|(id, serial)| after.get(*id) != Some(*serial))
        .count();
    println!("rename_native_reopens={reopened}; before={before:?}; after={after:?}");
    assert_eq!(
        after, before,
        "a metadata rename must retain every PCM owner"
    );
    assert_pcm(&mut direct);
    assert_pcm(&mut forwarded);
    assert_native_name(A, "Renamed shared bus");
    assert_catalog_name(&session, "Renamed shared bus");
    session.catalog(GraphCatalogAction::Select { id: graph_b });
    for name in ["공유 버스", "Shared from graph B"] {
        rename_document(&session, name);
        assert_native_name(A, name);
        assert_catalog_name(&session, name);
        assert_eq!(inventory(), before);
        assert_pcm(&mut direct);
        assert_pcm(&mut forwarded);
    }
    assert_native_name(B, &device_spec(B).display_name);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one native ownership scenario verifies transfer, rollback and final retirement together"
)]
fn native_rename_rollback_and_retired_controls_never_retarget() {
    let _driver_devices = DriverFixture::new();
    let _driver = crate::test_support::ObservedDriver::new();
    let format = render_format();
    let original = specs(vec![("a".into(), device_spec(A))]).unwrap();
    let active = EndpointSet::prepare(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &original,
        &EndpointSpecs::new(),
        format,
        format,
    )
    .unwrap();
    let controls = active.controls(&EndpointControls::default()).unwrap();
    let stale = active.nodes["a"]
        .owner
        .as_ref()
        .unwrap()
        .input
        .as_ref()
        .unwrap()
        .properties();
    let serials = inventory();
    let mut renamed = original.clone();
    renamed.get_mut("a").unwrap().display_name = "Pending name".into();
    let edit = EndpointEdit::prepare(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &renamed,
        &original,
        &controls,
        format,
    )
    .unwrap();
    let rejected = edit.publish(|_| {
        Err(HostAudioBackendError::BackendFailure {
            message: "publication rejected".into(),
        })
    });
    assert!(
        rejected
            .err()
            .unwrap()
            .to_string()
            .contains("publication rejected")
    );
    assert_native_name(A, &device_spec(A).display_name);
    assert_eq!(inventory(), serials);

    let cancelled = EndpointEdit::prepare(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &renamed,
        &original,
        &controls,
        format,
    )
    .unwrap();
    drop(cancelled);
    renamed.get_mut("a").unwrap().display_name = "invalid\0name".into();
    assert!(
        EndpointEdit::prepare(
            &mixrelay_linux_pipewire::StreamOwners::default(),
            &renamed,
            &original,
            &controls,
            format
        )
        .is_err()
    );
    assert_native_name(A, &device_spec(A).display_name);
    drop(active);
    assert_eq!(
        inventory(),
        serials,
        "Driver devices must outlive graph clients"
    );
    assert!(stale.description_change("before", "stale").is_err());
    let replacement = EndpointSet::prepare(
        &mixrelay_linux_pipewire::StreamOwners::default(),
        &original,
        &EndpointSpecs::new(),
        format,
        format,
    )
    .unwrap();
    assert!(stale.description_change("before", "stale").is_err());
    assert_native_name(A, &device_spec(A).display_name);
    assert_eq!(inventory(), serials);
    drop(replacement);
    assert_eq!(inventory(), serials);
    println!(
        "rename_rollback=passed; cancellation=passed; expired_capability_retarget=0; driver_nodes_after_retirement=4"
    );
}
