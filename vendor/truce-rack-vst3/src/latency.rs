//! Publish latency snapshots only for a serviced controller revision.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LatencyServiceAction {
    Restart,
    Refresh,
}

pub(super) fn latency_service_action(latency_changed: bool) -> LatencyServiceAction {
    if latency_changed {
        LatencyServiceAction::Restart
    } else {
        LatencyServiceAction::Refresh
    }
}

pub(super) fn latency_refresh_allowed(
    activation_ready: bool,
    editor_open: bool,
    pending_changes: bool,
) -> bool {
    activation_ready && !editor_open && !pending_changes
}

pub(super) fn refresh_latency_cache(
    snapshot: &mut Option<u32>,
    revision: u64,
    query: impl FnOnce() -> u32,
    publish: impl FnOnce(u64) -> bool,
) -> bool {
    let frames = query();
    if !publish(revision) {
        *snapshot = None;
        return false;
    }
    *snapshot = Some(frames);
    true
}

pub(super) fn latency_revision_is_current(
    captured_revision: u64,
    current_revision: u64,
    pending: bool,
) -> bool {
    captured_revision == current_revision && !pending
}

#[cfg(test)]
mod tests;
