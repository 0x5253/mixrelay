use super::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

#[test]
fn editor_process_fixture() {
    let Some(executable) = std::env::var_os("MIXRELAY_FIXTURE_CONTROLLER") else {
        return;
    };
    spawn_controller(std::path::Path::new(&executable)).unwrap();
}

#[test]
fn controller_survives_actual_editor_process_exit() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("controller.sh");
    let ready = directory.path().join("ready");
    let stop = directory.path().join("stop");
    let finished = directory.path().join("finished");
    // A bounded fixture always exits even if an assertion fails; no native audio is opened.
    fs::write(&executable, "#!/bin/sh\ncd \"$(dirname \"$0\")\"\nprintf ready > ready\ni=0\nwhile test ! -f stop && test \"$i\" -lt 100; do sleep 0.05; i=$((i+1)); done\nprintf finished > finished\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background::process::tests::editor_process_fixture",
        ])
        .env("MIXRELAY_FIXTURE_CONTROLLER", &executable)
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !finished.exists(),
        "editor exit must not stop the controller"
    );
    fs::write(stop, "stop").unwrap();
    while !finished.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}
