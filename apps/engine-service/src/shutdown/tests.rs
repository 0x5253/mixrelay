use super::*;

#[test]
fn cleanup_collects_every_failed_owner_in_retirement_order() {
    let errors = collect([
        ("control", Err("listener failed".into())),
        ("observers", Ok(())),
        ("worker", Err("render failed".into())),
        ("recorder", Err("finalization failed".into())),
    ])
    .unwrap_err();
    assert_eq!(
        errors,
        "control: listener failed\nworker: render failed\nrecorder: finalization failed"
    );
    assert!(collect([("control", Ok(())), ("worker", Ok(()))]).is_ok());
}

#[test]
fn only_a_clean_run_clears_the_crash_marker() {
    let directory = mixrelay_test_support::FixtureDir::new("crash-guard").unwrap();
    // The guard records each run beside the graph catalog in the configuration directory.
    let marker = directory.join("engine-run.json");
    let (guard, _) = CrashGuard::arm(Ok(directory.path().to_owned()));
    assert!(marker.exists());
    finish_crash_guard(guard, &Err("audio_join: worker panicked".into()));
    assert!(
        marker.exists(),
        "a failed run must stay counted as an unexpected end"
    );
    let (guard, _) = CrashGuard::arm(Ok(directory.path().to_owned()));
    finish_crash_guard(guard, &Ok(()));
    assert!(!marker.exists());
}
