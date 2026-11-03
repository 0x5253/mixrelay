use super::*;

#[derive(Debug)]
struct Owner {
    name: String,
    dropped: Arc<Mutex<Vec<String>>>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.dropped.lock().unwrap().push(self.name.clone());
    }
}

fn manifest(revision: u64, selectors: &[(&str, &str)]) -> ApplicationCaptureManifest {
    serde_json::from_value(serde_json::json!({
        "protocolVersion":PROTOCOL_VERSION, "revision":revision,
        "sources":selectors.iter().map(|(id, bundle)| serde_json::json!({
            "nodeId":id, "request":{
                "selector":{"match":"bundle_id", "bundle_id":bundle,"include_descendants":false},
                "stream":{"channels":2,"channelLayout":"stereo"}, "sourcePlayback":"preserve"
            }
        })).collect::<Vec<_>>()
    }))
    .unwrap()
}

fn owner(name: &str, dropped: &Arc<Mutex<Vec<String>>>) -> Arc<Owner> {
    Arc::new(Owner {
        name: name.to_owned(),
        dropped: Arc::clone(dropped),
    })
}

fn initial(dropped: &Arc<Mutex<Vec<String>>>) -> CaptureSet<Owner> {
    prepare::<Owner, String>(&manifest(1, &[("app", "old")]), None, &[], |_, _, _| {
        Ok::<_, String>(owner("old", dropped))
    })
    .unwrap()
}

#[test]
fn failed_prepare_drops_provisional_owners_without_removing_active_capture() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let active = Some(initial(&dropped));
    let proposed = manifest(2, &[("app", "replacement"), ("second", "unavailable")]);
    let failed = prepare::<Owner, String>(&proposed, active.as_ref(), &[], |id, _, previous| {
        if id.as_str() == "second" {
            return Err("native tap open failed".to_owned());
        }
        assert_eq!(previous.unwrap().name, "old");
        Ok::<_, String>(owner("provisional", &dropped))
    });
    assert!(failed.is_err());
    assert_eq!(active.as_ref().unwrap().revision, 1);
    assert_eq!(active.as_ref().unwrap().sources[0].session.name, "old");
    assert_eq!(*dropped.lock().unwrap(), ["provisional"]);
    drop(active);
    assert_eq!(*dropped.lock().unwrap(), ["provisional", "old"]);
}

#[test]
fn remote_graph_rejection_drops_only_prepared_replacement_and_added_node() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let active = Some(initial(&dropped));
    let proposed = manifest(2, &[("app", "replacement"), ("second", "added")]);
    let pending = prepare::<Owner, String>(&proposed, active.as_ref(), &[], |id, _, _| {
        Ok::<_, String>(owner(id.as_str(), &dropped))
    })
    .unwrap();
    assert!(dropped.lock().unwrap().is_empty());
    // The HTTP reject/error path drops its token without calling commit.
    drop(pending);
    assert_eq!(*dropped.lock().unwrap(), ["app", "second"]);
    assert_eq!(active.as_ref().unwrap().sources[0].session.name, "old");
}

#[test]
fn accepted_replacement_activates_once_then_retires_old_owner_and_removal_releases_new_owner() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut active = Some(initial(&dropped));
    let pending = prepare::<Owner, String>(
        &manifest(2, &[("app", "new")]),
        active.as_ref(),
        &[],
        |_, _, _| Ok::<_, String>(owner("new", &dropped)),
    )
    .unwrap();
    let mut activated = Vec::new();
    let retired = commit(&mut active, pending, |session| {
        activated.push(session.name.clone());
    });
    assert_eq!(activated, ["new"]);
    assert!(dropped.lock().unwrap().is_empty());
    assert_eq!(active.as_ref().unwrap().revision, 2);
    drop(retired);
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
    let removed = prepare::<Owner, String>(
        &manifest(3, &[]),
        active.as_ref(),
        &[],
        |_, _, _| unreachable!(),
    )
    .unwrap();
    drop(commit(&mut active, removed, |_| {
        panic!("removal cannot activate")
    }));
    assert_eq!(*dropped.lock().unwrap(), ["old", "new"]);
    assert!(active.as_ref().unwrap().sources.is_empty());
}

#[test]
fn unchanged_capture_reuses_arc_and_rejected_token_cannot_stop_it() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut active = Some(initial(&dropped));
    let pending = prepare::<Owner, String>(
        &manifest(2, &[("app", "old")]),
        active.as_ref(),
        &[],
        |_, _, _| panic!("must reuse"),
    )
    .unwrap();
    assert!(Arc::ptr_eq(
        &active.as_ref().unwrap().sources[0].session,
        &pending.sources[0].session
    ));
    drop(pending);
    assert!(dropped.lock().unwrap().is_empty());
    let accepted = prepare::<Owner, String>(
        &manifest(3, &[("app", "old")]),
        active.as_ref(),
        &[],
        |_, _, _| panic!("must reuse"),
    )
    .unwrap();
    drop(commit(&mut active, accepted, |_| {
        panic!("reused capture cannot restart")
    }));
    assert!(dropped.lock().unwrap().is_empty());
    drop(active);
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
}

#[test]
fn scoped_playback_renews_only_on_a_new_explicit_manifest() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut first = manifest(1, &[("app", "old")]);
    first.sources[0].request.source_playback =
        mixrelay_audio_graph::SourcePlaybackPolicy::MuteWhileCaptured;
    first.sources[0].request.forwarding_endpoint =
        Some("17e59d73-42c1-4f11-a8b4-6c2d913357e0".into());
    let active = prepare::<Owner, String>(&first, None, &[], |_, _, _| {
        Ok::<_, String>(owner("first", &dropped))
    })
    .unwrap();
    let retry =
        prepare::<Owner, String>(&first, Some(&active), &[], |_, _, _| panic!("same lease"))
            .unwrap();
    assert!(Arc::ptr_eq(
        &active.sources[0].session,
        &retry.sources[0].session
    ));
    let mut next = first.clone();
    next.revision = 2;
    let replacement = prepare::<Owner, String>(&next, Some(&active), &[], |_, _, previous| {
        assert!(Arc::ptr_eq(previous.unwrap(), &active.sources[0].session));
        Ok::<_, String>(owner("renewed", &dropped))
    })
    .unwrap();
    assert!(!Arc::ptr_eq(
        &active.sources[0].session,
        &replacement.sources[0].session
    ));
    assert!(replacement.sources[0].activate);
}

#[test]
fn mailbox_retains_old_reader_until_commit_and_worker_handoff() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mailbox = CaptureHandoff::default();
    let mut worker_capture = owner("old", &dropped);
    let prepared = owner("new", &dropped);
    assert!(mailbox.try_take().is_none());
    assert_eq!(worker_capture.name, "old");
    mailbox.publish(prepared);
    assert_eq!(worker_capture.name, "old");
    assert!(dropped.lock().unwrap().is_empty());
    worker_capture = mailbox.try_take().unwrap();
    assert_eq!(worker_capture.name, "new");
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
    assert!(mailbox.try_take().is_none());
}

#[test]
fn consecutive_commits_coalesce_without_leaking_intermediate_capture() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mailbox = CaptureHandoff::default();
    mailbox.publish(owner("intermediate", &dropped));
    mailbox.publish(owner("latest", &dropped));
    assert_eq!(*dropped.lock().unwrap(), ["intermediate"]);
    let worker_capture = mailbox.try_take().unwrap();
    assert_eq!(worker_capture.name, "latest");
    drop(mailbox);
    assert_eq!(*dropped.lock().unwrap(), ["intermediate"]);
    drop(worker_capture);
    assert_eq!(*dropped.lock().unwrap(), ["intermediate", "latest"]);
}

#[test]
fn pending_acceptance_keeps_old_capture_until_matching_audio_ack() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners {
        active: Some(initial(&dropped)),
        ..CaptureOwners::default()
    };
    let prepared = prepare::<Owner, String>(
        &manifest(2, &[("new-source", "new")]),
        owners.latest(),
        &[],
        |_, _, _| Ok::<_, String>(owner("new", &dropped)),
    )
    .unwrap();
    drop(owners.stage(prepared).unwrap());
    assert!(owners.acknowledge(1, |_| panic!("not active")).is_empty());
    assert!(dropped.lock().unwrap().is_empty());
    drop(owners.acknowledge(2, |_| {}));
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
    assert_eq!(
        owners.active.as_ref().unwrap().sources[0].node_id,
        "new-source"
    );
}

#[test]
fn repeated_pending_manifest_keeps_replacement_handoff_and_does_not_open_again() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners {
        active: Some(initial(&dropped)),
        ..CaptureOwners::default()
    };
    let proposed = manifest(2, &[("app", "new")]);
    let prepared = prepare::<Owner, String>(&proposed, owners.latest(), &[], |_, _, _| {
        Ok::<_, String>(owner("new", &dropped))
    })
    .unwrap();
    drop(owners.stage(prepared).unwrap());
    let prepared = prepare::<Owner, String>(&proposed, owners.latest(), &[], |_, _, _| {
        panic!("duplicate tap")
    })
    .unwrap();
    drop(owners.stage(prepared).unwrap());
    let mut activations = Vec::new();
    drop(owners.acknowledge(2, |capture| activations.push(capture.name.clone())));
    assert_eq!(activations, ["new"]);
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
    assert!(
        owners
            .acknowledge(2, |_| panic!("duplicate handoff"))
            .is_empty()
    );
}

#[test]
fn removal_and_readdition_before_ack_reuses_retained_native_owner() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners {
        active: Some(initial(&dropped)),
        ..CaptureOwners::default()
    };
    let removal = prepare::<Owner, String>(
        &manifest(2, &[]),
        owners.latest(),
        &[],
        |_, _, _| unreachable!(),
    )
    .unwrap();
    drop(owners.stage(removal).unwrap());
    let retained = owners.retained().unwrap();
    let readded = prepare::<Owner, String>(
        &manifest(3, &[("app", "old")]),
        owners.latest(),
        &retained,
        |_, _, _| panic!("cannot steal a retained producer lease"),
    )
    .unwrap();
    drop(owners.stage(readded).unwrap());
    drop(owners.acknowledge(3, |_| panic!("unchanged tap")));
    assert!(dropped.lock().unwrap().is_empty());
    let final_removal = prepare::<Owner, String>(
        &manifest(4, &[]),
        owners.latest(),
        &[],
        |_, _, _| unreachable!(),
    )
    .unwrap();
    drop(owners.stage(final_removal).unwrap());
    assert!(dropped.lock().unwrap().is_empty());
    drop(owners.acknowledge(4, |_| {}));
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
}

#[test]
fn superseded_pending_source_lives_until_newest_revision_ack() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners::<Owner>::default();
    for revision in [1, 2] {
        let id = format!("source-{revision}");
        let prepared = prepare::<Owner, String>(
            &manifest(revision, &[(&id, &id)]),
            owners.latest(),
            &[],
            |_, _, _| Ok::<_, String>(owner(&id, &dropped)),
        )
        .unwrap();
        drop(owners.stage(prepared).unwrap());
    }
    assert!(dropped.lock().unwrap().is_empty());
    drop(owners.acknowledge(1, |_| {}));
    assert!(dropped.lock().unwrap().is_empty());
    drop(owners.acknowledge(2, |_| {}));
    assert_eq!(*dropped.lock().unwrap(), ["source-1"]);
}

#[test]
fn invalid_manifest_is_rejected_before_any_native_owner_opens() {
    let mut duplicate = manifest(1, &[("same", "a"), ("same", "b")]);
    assert!(
        prepare::<Owner, String>(&duplicate, None, &[], |_, _, _| panic!("invalid manifest"))
            .is_err()
    );
    duplicate.sources.pop();
    duplicate.sources[0].node_id = "x".repeat(128);
    assert!(
        prepare::<Owner, String>(&duplicate, None, &[], |_, _, _| panic!("invalid manifest"))
            .is_err()
    );
    duplicate.sources[0].node_id = "valid".into();
    duplicate.sources[0].request.gain_db = 1.0;
    assert!(
        prepare::<Owner, String>(&duplicate, None, &[], |_, _, _| panic!("invalid manifest"))
            .is_err()
    );
    duplicate.sources[0].request.gain_db = 0.0;
    duplicate.sources[0].request.selector = ApplicationSelector::BundleId {
        bundle_id: "exact".into(),
        include_descendants: true,
    };
    assert!(
        prepare::<Owner, String>(&duplicate, None, &[], |_, _, _| panic!(
            "unsupported expansion"
        ))
        .is_err()
    );
}

#[test]
fn stale_or_conflicting_same_revision_cannot_replace_accepted_owners() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners {
        active: Some(initial(&dropped)),
        ..CaptureOwners::default()
    };
    for proposed in [manifest(0, &[]), manifest(1, &[("other", "different")])] {
        let prepared = prepare::<Owner, String>(&proposed, owners.latest(), &[], |_, _, _| {
            Ok::<_, String>(owner("rejected", &dropped))
        })
        .unwrap();
        assert!(owners.stage(prepared).is_err());
        assert_eq!(owners.latest().unwrap().revision, 1);
        assert_eq!(owners.latest().unwrap().sources[0].session.name, "old");
    }
    assert_eq!(*dropped.lock().unwrap(), ["rejected"]);
}

#[test]
fn a_realtime_consumer_never_waits_for_a_publisher() {
    let handoff = CaptureHandoff::default();
    handoff.publish(7);
    let held = handoff.0.lock().unwrap();
    // Contended: nothing is taken, and the published value stays for a later pass.
    assert_eq!(handoff.try_take(), None);
    drop(held);
    assert_eq!(handoff.try_take(), Some(7));
    assert_eq!(handoff.try_take(), None);
}

#[test]
fn failed_successor_start_rolls_back_only_new_starts_and_keeps_owners_for_retry() {
    let dropped = Arc::new(Mutex::new(Vec::new()));
    let mut owners = CaptureOwners {
        active: Some(initial(&dropped)),
        ..CaptureOwners::default()
    };
    let next = prepare::<Owner, std::io::Error>(
        &manifest(
            2,
            &[("first", "first"), ("second", "second"), ("last", "last")],
        ),
        owners.latest(),
        &[],
        |id, _, _| Ok(owner(id.as_str(), &dropped)),
    )
    .unwrap();
    drop(owners.stage(next).unwrap());
    let mut starts = Vec::new();
    let mut rollbacks = Vec::new();
    let failure = owners
        .acknowledge_started(
            2,
            |session| {
                starts.push(session.name.clone());
                if session.name == "last" {
                    return Err(std::io::Error::other("start last"));
                }
                Ok((session.name == "first").then(|| session.name.clone()))
            },
            |token| {
                rollbacks.push(token);
                Err(std::io::Error::other("restore first"))
            },
            |_| panic!("failed start cannot hand off"),
        )
        .unwrap_err();
    assert_eq!(starts, ["first", "second", "last"]);
    assert_eq!(rollbacks, ["first"]);
    assert_eq!(failure.start.to_string(), "start last");
    assert_eq!(failure.rollback.len(), 1);
    assert_eq!(failure.rollback[0].to_string(), "restore first");
    assert_eq!(owners.active.as_ref().unwrap().revision, 1);
    assert!(owners.has_unacknowledged());
    assert!(dropped.lock().unwrap().is_empty());

    let mut activated = Vec::new();
    let retired = owners
        .acknowledge_started(
            2,
            |_| Ok::<Option<()>, std::io::Error>(None),
            |()| unreachable!(),
            |session| activated.push(session.name.clone()),
        )
        .unwrap();
    // Every new source has no scoped playback/replacement in this manifest.
    assert!(activated.is_empty());
    assert_eq!(owners.active.as_ref().unwrap().revision, 2);
    assert!(!owners.has_unacknowledged());
    assert!(dropped.lock().unwrap().is_empty());
    drop(retired);
    assert_eq!(*dropped.lock().unwrap(), ["old"]);
}
