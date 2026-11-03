//! Preserve failures after every independent shutdown owner has finished.
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
