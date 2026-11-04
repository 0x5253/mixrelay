//! Retirement steps both native services share: every owner is retired before any failure
//! is reported, and only a fully clean run counts as an expected exit.
use mixrelay_engine_host::crash_guard::CrashGuard;

/// Combines failures from shutdown owners after every owner has been retired.
pub(crate) fn collect(
    outcomes: impl IntoIterator<Item = (&'static str, Result<(), String>)>,
) -> Result<(), String> {
    let errors = outcomes
        .into_iter()
        .filter_map(|(boundary, outcome)| outcome.err().map(|error| format!("{boundary}: {error}")))
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// Clears the crash marker only for a run whose every owner retired cleanly, so a failed
/// run still counts toward starting the next run paused.
pub(crate) fn finish_crash_guard(guard: Option<CrashGuard>, run: &Result<(), String>) {
    if run.is_ok()
        && let Some(guard) = guard
    {
        guard.finish();
    }
}

#[cfg(test)]
mod tests;
