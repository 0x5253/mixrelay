use super::*;
use mixrelay_audio_io::{ApplicationSelector, ChannelLayout, SourcePlaybackPolicy, StreamSpec};

fn prepared(
    ids: &[&str],
    generation: u64,
    previous: Option<&CaptureWatch>,
) -> (CaptureSet, CaptureWatch) {
    let requests = ids
        .iter()
        .map(|id| {
            (
                (*id).into(),
                ApplicationCaptureRequest {
                    selector: ApplicationSelector::BundleId {
                        bundle_id: (*id).into(),
                        include_descendants: false,
                    },
                    stream: StreamSpec {
                        channels: 2,
                        channel_layout: ChannelLayout::Stereo,
                    },
                    source_playback: SourcePlaybackPolicy::Preserve,
                    forwarding_endpoint: None,
                    gain_db: 0.0,
                    requires_reselection: false,
                },
            )
        })
        .collect::<Vec<_>>();
    CaptureSet::open_inventory_format(
        &mixrelay_pipewire::StreamOwners::default(),
        &requests,
        &[],
        &Preparation::new(generation, super::super::super::render_format(), previous),
    )
    .unwrap()
}

fn activate(current: &mut CaptureSet, mut next: CaptureSet) {
    assert!(next.can_retain_from(current));
    let allocation = allocation_counter::measure(|| {
        next.retain_from(current);
        std::mem::swap(current, &mut next);
    });
    assert_eq!(allocation.count_total, 0);
    assert_eq!(allocation.bytes_total, 0);
    assert_eq!(
        allocation.count_current, 0,
        "activation must not free native storage either"
    );
    // The displaced envelope is dropped here, outside the measured RT boundary.
}

fn pending_recovery(watch: &CaptureWatch) -> CaptureSet {
    let entry = &watch.entries[0];
    let mut capture =
        super::super::prepare(entry, &[], &std::sync::atomic::AtomicBool::new(false)).unwrap();
    capture.scratch.fill(0.75);
    CaptureSet {
        generation: watch.generation,
        nodes: BTreeMap::from([(entry.node_id.clone(), capture)]),
        ..CaptureSet::default()
    }
}

#[test]
fn retained_source_keeps_storage_health_and_lease_without_rt_allocation() {
    let (mut active, watch) = prepared(&["keep"], 1, None);
    active.nodes.get_mut("keep").unwrap().scratch.fill(0.25);
    let scratch = active.nodes["keep"].scratch.as_ptr();
    let health = active.nodes["keep"].healthy.clone();
    let (next, next_watch) = prepared(&["keep", "add"], 2, Some(&watch));
    assert!(next.nodes["keep"].scratch.is_empty());
    activate(&mut active, next);
    assert_eq!(active.nodes["keep"].scratch.as_ptr(), scratch);
    assert!(
        active.nodes["keep"]
            .scratch
            .iter()
            .all(|value| value.to_bits() == 0.25_f32.to_bits())
    );
    assert!(std::sync::Arc::ptr_eq(
        &health,
        &active.nodes["keep"].healthy
    ));
    assert_eq!(active.nodes["keep"].lease, CaptureLease(1));
    assert_eq!(active.nodes["add"].lease, CaptureLease(2));
    let (removed, _) = prepared(&["keep"], 3, Some(&next_watch));
    activate(&mut active, removed);
    assert_eq!(active.nodes["keep"].scratch.as_ptr(), scratch);
    assert_eq!(active.generation, 1);
}

#[test]
fn pending_recovery_survives_unrelated_edits_but_cannot_revive_a_new_lease() {
    let (mut active, watch) = prepared(&["keep"], 1, None);
    let mut recovery = pending_recovery(&watch);
    let (next, watch) = prepared(&["keep", "add"], 2, Some(&watch));
    activate(&mut active, next);
    assert_eq!(recovery.generation, active.generation);
    active.replace(&mut recovery);
    assert_eq!(
        active.nodes["keep"].scratch[0].to_bits(),
        0.75_f32.to_bits()
    );
    let mut stale = pending_recovery(&watch);
    let (removed, empty_watch) = prepared(&[], 3, Some(&watch));
    activate(&mut active, removed);
    let (readded, _) = prepared(&["keep"], 4, Some(&empty_watch));
    activate(&mut active, readded);
    active.replace(&mut stale);
    assert_eq!(active.nodes["keep"].lease, CaptureLease(4));
    assert_eq!(active.nodes["keep"].scratch[0].to_bits(), 0.0_f32.to_bits());
}

#[test]
fn watcher_refresh_merges_existing_leases_without_losing_new_or_restoring_removed_nodes() {
    let (_, original) = prepared(&["keep"], 1, None);
    let (_, mut updated) = prepared(&["keep", "add"], 2, Some(&original));
    let mut recovered = original.clone();
    recovered.entries[0].serials = vec!["recovered-native-stream".into()];
    updated.refresh_leases(&recovered);
    assert_eq!(updated.entries.len(), 2);
    assert_eq!(
        updated.by_node()["keep"].serials,
        recovered.entries[0].serials
    );
    assert_eq!(updated.by_node()["add"].lease, CaptureLease(2));
    let (_, removed) = prepared(&[], 3, Some(&updated));
    let (_, mut readded) = prepared(&["keep"], 4, Some(&removed));
    readded.refresh_leases(&recovered);
    assert!(readded.entries[0].serials.is_empty());
    assert_eq!(readded.entries[0].lease, CaptureLease(4));
}

#[test]
fn format_and_lease_mismatch_cannot_transfer_retained_owners() {
    let (active, watch) = prepared(&["keep"], 1, None);
    let (next, _) = prepared(&["keep"], 2, Some(&watch));
    assert!(!next.can_retain_from(&CaptureSet::default()));
    let (different_lease, _) = prepared(&["keep"], 3, None);
    assert!(!next.can_retain_from(&different_lease));
    assert_eq!(active.nodes["keep"].lease, CaptureLease(1));
    let format = RenderFormat {
        sample_rate: 44_100,
        block_frames: 256,
    };
    let changed = Preparation::new(4, format, Some(&watch));
    assert_eq!(changed.epoch, 4);
    assert!(changed.previous.is_empty());
}
