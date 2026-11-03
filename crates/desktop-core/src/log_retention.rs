//! Selection of obsolete logs from supplied file and process observations.

/// One observed log candidate; ordering keys are supplied by the native filesystem adapter.
pub struct LogEntry<T, K> {
    pub name: String,
    pub modified: K,
    pub live: bool,
    pub token: T,
}

/// Parses only process-owned role logs; malformed names never authorize removal.
#[must_use]
pub fn log_pid(name: &str, role: &str) -> Option<u32> {
    name.strip_prefix(role)?
        .strip_prefix('-')?
        .strip_suffix(".log")?
        .parse()
        .ok()
}

/// Keeps live processes and the newest exited logs, reserving one slot for a fresh log.
pub fn obsolete_logs<T, K: Ord>(
    role: &str,
    keep: usize,
    entries: impl IntoIterator<Item = LogEntry<T, K>>,
) -> Vec<T> {
    let mut logs: Vec<_> = entries
        .into_iter()
        .filter(|entry| !entry.live && log_pid(&entry.name, role).is_some())
        .collect();
    logs.sort_by(|left, right| right.modified.cmp(&left.modified));
    logs.into_iter()
        .skip(keep.saturating_sub(1))
        .map(|entry| entry.token)
        .collect()
}

#[cfg(test)]
mod tests;
