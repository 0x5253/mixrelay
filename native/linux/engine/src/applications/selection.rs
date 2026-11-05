//! Which registry streams belong to a capture selector. Process facts come from the
//! kernel through `lookup`, never from the application's own stream properties.
use mixrelay_audio_graph::ApplicationSelector;
use mixrelay_linux_audio_io::{
    PipeWireTarget,
    process_identity::{ProcessIdentity, same_process},
};

pub(super) fn matching_targets<'a>(
    inventory: &'a [PipeWireTarget],
    selector: &ApplicationSelector,
    pin: Option<&ProcessIdentity>,
    lookup: impl Fn(u32) -> Option<ProcessIdentity>,
) -> Vec<&'a PipeWireTarget> {
    let pinned_alive = pin.is_some_and(|pin| {
        lookup(pin.process_id).is_some_and(|current| same_process(pin, &current))
    });
    let matches = inventory
        .iter()
        .filter(|target| {
            target.media_class == "Stream/Output/Audio"
                && target.process_id != Some(std::process::id())
        })
        .filter(|target| match selector {
            ApplicationSelector::BundleId { bundle_id, .. } => {
                target.application_id.as_deref() == Some(bundle_id)
            }
            ApplicationSelector::ExecutablePath { path } => {
                executable_matches(target, path, &lookup)
            }
            ApplicationSelector::ProcessId {
                process_id,
                include_descendants,
            } => {
                pinned_alive
                    && target.process_id.is_some_and(|pid| {
                        pid == *process_id
                            || (*include_descendants && descendant(pid, *process_id, &lookup))
                    })
            }
            ApplicationSelector::AllExcept { bundle_ids } => !target
                .application_id
                .as_ref()
                .or(target.executable.as_ref())
                .is_some_and(|id| bundle_ids.contains(id)),
        })
        .collect();
    // Recheck after ancestry walking: an exit/reused root during discovery may
    // not transfer the legacy process intent to a different process family.
    if pin.is_some_and(|pin| {
        lookup(pin.process_id).is_none_or(|current| !same_process(pin, &current))
    }) {
        Vec::new()
    } else {
        matches
    }
}

/// The kernel's executable path must equal the selected path, and the stream's own
/// claim must name the same executable, in full or by file name.
fn executable_matches(
    target: &PipeWireTarget,
    path: &str,
    lookup: &impl Fn(u32) -> Option<ProcessIdentity>,
) -> bool {
    target
        .process_id
        .and_then(lookup)
        .and_then(|process| process.executable_path)
        .is_some_and(|actual| {
            actual == path
                && target.executable.as_ref().is_some_and(|reported| {
                    reported == &actual
                        || std::path::Path::new(&actual)
                            .file_name()
                            .and_then(|name| name.to_str())
                            == Some(reported)
                })
        })
}

/// Walks at most 64 parents; init, a self-parented process or a vanished one ends it.
fn descendant(
    mut candidate: u32,
    ancestor: u32,
    lookup: &impl Fn(u32) -> Option<ProcessIdentity>,
) -> bool {
    for _ in 0..64 {
        let Some(identity) = lookup(candidate) else {
            return false;
        };
        if identity.parent_id == ancestor {
            return true;
        }
        if identity.parent_id <= 1 || identity.parent_id == candidate {
            return false;
        }
        candidate = identity.parent_id;
    }
    false
}

/// Sorted, deduplicated registry serials: the identity of a capture's stream set.
pub(super) fn serials(targets: &[&PipeWireTarget]) -> Vec<String> {
    let mut serials = targets
        .iter()
        .map(|target| target.serial.clone())
        .collect::<Vec<_>>();
    serials.sort_unstable();
    serials.dedup();
    serials
}
