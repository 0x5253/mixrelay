use super::*;

fn locator(format: RackFormat, id: &str) -> RackLocator {
    serde_json::from_value(serde_json::json!({
        "format":format.argument(), "uniqueId":id, "path":format!("/fixture/{id}"),
        "name":id, "vendor":"fixture", "version":1, "category":"effect",
        "hasEditor":false, "acceptsMidi":false
    }))
    .unwrap()
}

fn locator_at(format: RackFormat, id: &str, path: &str) -> RackLocator {
    let mut locator = locator(format, id);
    locator.path = PathBuf::from(path);
    locator
}

fn record(locator: &RackLocator) -> rack_formats::RackProbeRecord {
    rack_formats::RackProbeRecord {
        descriptor: mixrelay_control_protocol::ExternalProcessorDescriptor {
            type_id: format!("{}:{}", locator.format.argument(), locator.unique_id),
            display_name: locator.unique_id.clone(),
            vendor: "fixture".to_owned(),
            version: "1".to_owned(),
            format: locator.format.protocol(),
            layouts: vec![mixrelay_audio_graph::ProviderPorts::canonical_stereo(
                "fixture-stereo",
            )],
            default_layout_id: "fixture-stereo".into(),
            has_editor: false,
            parameters: Vec::new(),
        },
        locator: locator.clone(),
    }
}

fn scan_ok(locators: Vec<RackLocator>) -> rack_formats::RackScanReport {
    rack_formats::RackScanReport {
        locators,
        path_failures: Vec::new(),
    }
}

#[test]
fn failed_probe_keeps_display_name_separate_from_vendor_diagnostics() {
    let catalog = ProcessorCatalog::default();
    let candidate = locator(RackFormat::Vst3, "565354fixture");
    rescan_with(
        &catalog,
        &[RackFormat::Vst3],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |_| Err("PasteBoard: vendor log\nMixRelay processor probe failed: load rejected".into()),
    );
    let diagnostics = catalog.protocol_catalog().diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].plugin_name.as_deref(), Some("565354fixture"));
    assert!(diagnostics[0].message.contains("vendor log"));
}

#[test]
fn instruments_are_not_probed_or_published_as_effects() {
    let catalog = ProcessorCatalog::default();
    let instrument: RackLocator = serde_json::from_value(serde_json::json!({
        "format":"vst3", "uniqueId":"instrument", "path":"/fixture/instrument.vst3",
        "name":"Example Instrument", "vendor":"fixture", "version":1,
        "category":"instrument", "hasEditor":true, "acceptsMidi":true
    }))
    .unwrap();
    rescan_with(
        &catalog,
        &[RackFormat::Vst3],
        |_| Ok(scan_ok(vec![instrument.clone()])),
        |_| panic!("instruments must not reach instance validation"),
    );
    let result = catalog.protocol_catalog();
    assert!(result.processors.is_empty());
    assert!(result.diagnostics.is_empty());
}

fn assert_locator_path(catalog: &ProcessorCatalog, id: &str, expected: &str) {
    let snapshot = catalog
        .snapshot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(
        snapshot.entries[id].rack.locator.path,
        PathBuf::from(expected)
    );
}

#[test]
fn scan_child_protocol_requires_structured_results_and_path_failures() {
    let report = rack_formats::RackScanReport {
        locators: vec![locator(RackFormat::Clap, "default")],
        path_failures: vec![rack_formats::ScanPathFailure {
            path: PathBuf::from("/bad"),
            message: "permission denied".into(),
        }],
    };
    let encoded = serde_json::to_value(&report).unwrap();
    assert!(encoded["locators"].is_array());
    assert_eq!(encoded["pathFailures"][0]["path"], "/bad");
    let decoded: rack_formats::RackScanReport = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.locators.len(), 1);
    assert_eq!(decoded.path_failures.len(), 1);
    assert!(serde_json::from_value::<rack_formats::RackScanReport>(serde_json::json!([])).is_err());
}

#[test]
fn partial_path_failure_publishes_successful_candidates_and_scan_diagnostic() {
    let catalog = ProcessorCatalog::default();
    let default = locator(RackFormat::Clap, "default");
    let custom = locator(RackFormat::Clap, "custom");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| {
            Ok(rack_formats::RackScanReport {
                locators: vec![default.clone(), custom.clone()],
                path_failures: vec![rack_formats::ScanPathFailure {
                    path: PathBuf::from("/bad"),
                    message: "permission denied".into(),
                }],
            })
        },
        |candidate| Ok(record(candidate)),
    );
    let partial = catalog.protocol_catalog();
    assert_eq!(partial.processors.len(), 2);
    assert_eq!(partial.diagnostics.len(), 1);
    assert_eq!(partial.diagnostics[0].stage, ProcessorDiscoveryStage::Scan);
    assert!(partial.diagnostics[0].plugin_id.is_none());
    assert!(partial.diagnostics[0].message.contains("/bad"));
    assert!(partial.diagnostics[0].message.contains("permission denied"));

    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![default.clone(), custom.clone()])),
        |candidate| Ok(record(candidate)),
    );
    let recovered = catalog.protocol_catalog();
    assert_eq!(recovered.processors.len(), 2);
    assert!(recovered.diagnostics.is_empty());
}

#[test]
fn partial_path_failure_retains_only_failed_subtree_and_retry_refreshes_it() {
    let catalog = ProcessorCatalog::default();
    let initial = vec![
        locator_at(RackFormat::Clap, "duplicate", "/private/duplicate.clap"),
        locator_at(
            RackFormat::Clap,
            "retained",
            "/private/nested/retained.clap",
        ),
        locator_at(RackFormat::Clap, "sibling", "/private-other/sibling.clap"),
        locator_at(
            RackFormat::Vst3,
            "other-format",
            "/private/other-format.vst3",
        ),
    ];
    rescan_with(
        &catalog,
        &[RackFormat::Clap, RackFormat::Vst3],
        |_| Ok(scan_ok(initial.clone())),
        |candidate| Ok(record(candidate)),
    );

    let fresh_duplicate = locator_at(RackFormat::Clap, "duplicate", "/readable/duplicate.clap");
    rescan_with(
        &catalog,
        &[RackFormat::Clap, RackFormat::Vst3],
        |format| {
            if format == RackFormat::Clap {
                Ok(rack_formats::RackScanReport {
                    locators: vec![fresh_duplicate.clone()],
                    path_failures: vec![rack_formats::ScanPathFailure {
                        path: PathBuf::from("/private"),
                        message: "permission denied".into(),
                    }],
                })
            } else {
                Ok(scan_ok(Vec::new()))
            }
        },
        |candidate| Ok(record(candidate)),
    );
    let partial = catalog.protocol_catalog();
    assert_eq!(
        partial
            .processors
            .iter()
            .map(|entry| entry.type_id.as_str())
            .collect::<Vec<_>>(),
        vec!["clap:duplicate", "clap:retained"]
    );
    assert_locator_path(&catalog, "clap:duplicate", "/readable/duplicate.clap");
    assert_locator_path(&catalog, "clap:retained", "/private/nested/retained.clap");
    assert_eq!(partial.diagnostics.len(), 1);

    let recovered_duplicate = locator_at(RackFormat::Clap, "duplicate", "/private/duplicate.clap");
    rescan_with(
        &catalog,
        &[RackFormat::Clap, RackFormat::Vst3],
        |format| {
            Ok(scan_ok(if format == RackFormat::Clap {
                vec![recovered_duplicate.clone()]
            } else {
                Vec::new()
            }))
        },
        |candidate| Ok(record(candidate)),
    );
    let recovered = catalog.protocol_catalog();
    assert_eq!(recovered.processors.len(), 1);
    assert!(recovered.diagnostics.is_empty());
    assert_locator_path(&catalog, "clap:duplicate", "/private/duplicate.clap");
    let snapshot = catalog
        .snapshot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(!snapshot.entries.contains_key("clap:retained"));
}

#[test]
fn partial_path_failure_drops_a_successfully_reclassified_non_effect() {
    let catalog = ProcessorCatalog::default();
    let non_effect = locator_at(RackFormat::Clap, "non-effect", "/private/non-effect.clap");
    let retained = locator_at(RackFormat::Clap, "retained", "/private/retained.clap");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![non_effect.clone(), retained.clone()])),
        |candidate| Ok(record(candidate)),
    );

    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| {
            Ok(rack_formats::RackScanReport {
                locators: vec![non_effect.clone()],
                path_failures: vec![rack_formats::ScanPathFailure {
                    path: PathBuf::from("/private"),
                    message: "permission denied for a sibling".into(),
                }],
            })
        },
        |candidate| {
            let mut record = record(candidate);
            record.descriptor.layouts.clear();
            Ok(record)
        },
    );
    let catalog = catalog.protocol_catalog();
    assert_eq!(
        catalog
            .processors
            .iter()
            .map(|entry| entry.type_id.as_str())
            .collect::<Vec<_>>(),
        vec!["clap:retained"]
    );
    assert_eq!(catalog.diagnostics.len(), 1);
}

#[test]
fn successful_empty_is_distinct_from_failed_format_and_retry_clears_diagnostics() {
    let catalog = ProcessorCatalog::default();
    let candidate = locator(RackFormat::Clap, "effect");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |candidate| Ok(record(candidate)),
    );
    let initial = catalog.protocol_catalog();
    assert_eq!(initial.processors.len(), 1);
    for message in [
        "child timed out",
        "invalid child JSON",
        "child exited with 7",
    ] {
        rescan_with(
            &catalog,
            &[RackFormat::Clap],
            |_| Err(message.to_owned()),
            |_| unreachable!(),
        );
        let failed = catalog.protocol_catalog();
        assert_eq!(failed.processors, initial.processors);
        assert_eq!(failed.diagnostics.len(), 1);
        assert_eq!(failed.diagnostics[0].stage, ProcessorDiscoveryStage::Scan);
        assert_eq!(failed.diagnostics[0].message, message);
        assert!(failed.diagnostics[0].plugin_id.is_none());
    }
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(Vec::new())),
        |_| unreachable!(),
    );
    let retried = catalog.protocol_catalog();
    assert!(retried.processors.is_empty());
    assert!(retried.diagnostics.is_empty());
}

#[test]
fn permission_denial_preserves_one_catalog_and_recovery_reprobes_its_effect() {
    let catalog = ProcessorCatalog::default();
    let candidate = locator(RackFormat::Clap, "effect");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |locator| Ok(record(locator)),
    );
    let initial = catalog.protocol_catalog();
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Err("permission denied (os error 13)".into()),
        |_| panic!("a denied scan must not probe"),
    );
    let denied = catalog.protocol_catalog();
    assert_eq!(denied.processors, initial.processors);
    assert_eq!(denied.diagnostics.len(), 1);
    assert_eq!(denied.diagnostics[0].stage, ProcessorDiscoveryStage::Scan);
    assert!(denied.diagnostics[0].message.contains("permission denied"));
    let mut probes = 0;
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |locator| {
            probes += 1;
            let mut refreshed = record(locator);
            refreshed.descriptor.display_name = "fresh after permission recovery".into();
            Ok(refreshed)
        },
    );
    let recovered = catalog.protocol_catalog();
    assert_eq!(probes, 1);
    assert_eq!(recovered.processors.len(), 1);
    assert_eq!(
        recovered.processors[0].type_id,
        initial.processors[0].type_id
    );
    assert_eq!(
        recovered.processors[0].display_name,
        "fresh after permission recovery"
    );
    assert!(recovered.diagnostics.is_empty());
}

#[test]
fn partial_probe_failure_retains_only_exact_candidate_and_successful_formats_refresh() {
    let catalog = ProcessorCatalog::default();
    let failed = locator(RackFormat::Clap, "failed");
    let removed = locator(RackFormat::Clap, "removed");
    let new = locator(RackFormat::Vst3, "new");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![failed.clone(), removed.clone()])),
        |candidate| Ok(record(candidate)),
    );
    let mut probes = 0;
    rescan_with(
        &catalog,
        &[RackFormat::Clap, RackFormat::Vst3],
        |format| {
            Ok(scan_ok(if format == RackFormat::Clap {
                vec![failed.clone(), failed.clone()]
            } else {
                vec![new.clone()]
            }))
        },
        |candidate| {
            probes += 1;
            if candidate.unique_id == "failed" {
                Err("load failed".to_owned())
            } else {
                Ok(record(candidate))
            }
        },
    );
    let partial = catalog.protocol_catalog();
    assert_eq!(
        probes, 2,
        "overlapping default/custom paths probe an identity only once"
    );
    assert_eq!(
        partial
            .processors
            .iter()
            .map(|entry| entry.type_id.as_str())
            .collect::<Vec<_>>(),
        vec!["clap:failed", "vst3:new"]
    );
    assert_eq!(partial.diagnostics.len(), 1);
    assert_eq!(
        partial.diagnostics[0].plugin_id.as_deref(),
        Some("clap:failed")
    );
    assert_eq!(partial.diagnostics[0].stage, ProcessorDiscoveryStage::Probe);

    let mut replaced = failed.clone();
    replaced.path = PathBuf::from("/different/path");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![replaced.clone()])),
        |_| Err("load failed".to_owned()),
    );
    assert!(
        catalog.protocol_catalog().processors.is_empty(),
        "same ID from a different bundle must not retain a stale locator"
    );
}

#[test]
fn successful_probe_reclassification_removes_old_effect_without_a_failure() {
    let catalog = ProcessorCatalog::default();
    let candidate = locator(RackFormat::Clap, "effect");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |candidate| Ok(record(candidate)),
    );
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |candidate| {
            let mut record = record(candidate);
            record.descriptor.layouts.clear();
            Ok(record)
        },
    );
    assert!(catalog.protocol_catalog().processors.is_empty());
    assert!(catalog.protocol_catalog().diagnostics.is_empty());
}

#[test]
fn catalog_readers_keep_atomic_previous_snapshot_during_scan() {
    let catalog = ProcessorCatalog::default();
    let candidate = locator(RackFormat::Clap, "effect");
    rescan_with(
        &catalog,
        &[RackFormat::Clap],
        |_| Ok(scan_ok(vec![candidate.clone()])),
        |candidate| Ok(record(candidate)),
    );
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let owner = catalog.clone();
    let scan = std::thread::spawn(move || {
        rescan_with(
            &owner,
            &[RackFormat::Clap],
            |_| {
                entered_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Err("scan failed".to_owned())
            },
            |_| unreachable!(),
        );
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let during = catalog.protocol_catalog();
    assert_eq!(during.processors.len(), 1);
    assert!(during.diagnostics.is_empty());
    resume_tx.send(()).unwrap();
    scan.join().unwrap();
    let after = catalog.protocol_catalog();
    assert_eq!(after.processors, during.processors);
    assert_eq!(after.diagnostics.len(), 1);
}
