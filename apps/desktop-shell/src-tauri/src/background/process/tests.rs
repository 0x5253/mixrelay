use super::*;
use crate::background::fixture_pipe::Pipe;

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
    let mut ready = Pipe::create(&directory.path().join("ready"));
    let mut stop = Pipe::create(&directory.path().join("stop"));
    let mut finished = Pipe::create(&directory.path().join("finished"));
    // The fixture blocks only on `stop`, which the test holds open; no native audio is opened.
    mixrelay_test_support::write_executable(
        &executable,
        "#!/bin/sh\ncd \"${0%/*}\"\nprintf ready > ready\nread -r _ < stop\nprintf finished > finished\n",
    )
    .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "background::process::tests::editor_process_fixture",
        ])
        .env("MIXRELAY_FIXTURE_CONTROLLER", &executable)
        .status()
        .unwrap();
    assert!(status.success());
    ready.receive("ready");
    assert!(
        !finished.readable(0),
        "editor exit must not stop the controller"
    );
    stop.send("stop\n");
    finished.receive("finished");
}
