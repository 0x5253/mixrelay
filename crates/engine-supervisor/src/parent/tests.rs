//! Controller child startup, ownership loss and readiness failures with real processes.
use super::*;
use mixrelay_time::StdClock;
use std::{
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
mod unix_processes {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };
    use tempfile::{TempDir, tempdir};

    #[test]
    fn owned_fixture_starts_and_exits_after_lease_closure() {
        let (_directory, executable) = fixture("cat >/dev/null");
        let mut engine = start_fixture(&executable).expect("fixture should start");
        assert_ne!(engine.id(), 0);
        assert!(
            engine
                .try_wait()
                .expect("fixture status should be readable")
                .is_none()
        );

        engine.request_shutdown();
        let status = wait_for_exit(&mut engine);
        assert!(status.success());
    }

    #[test]
    fn closing_a_retired_child_lease_does_not_stop_its_successor() {
        let (_old_directory, old_executable) = fixture("cat >/dev/null");
        let (_new_directory, new_executable) = fixture("cat >/dev/null");
        let mut old = start_fixture(&old_executable).unwrap();
        let mut successor = start_fixture(&new_executable).unwrap();
        assert_ne!(old.id(), successor.id());
        old.request_shutdown();
        assert!(wait_for_exit(&mut old).success());
        assert!(successor.try_wait().unwrap().is_none());
        successor.request_shutdown();
        assert!(wait_for_exit(&mut successor).success());
    }

    #[test]
    fn reaper_failure_precedes_spawn_and_preserves_the_active_child() {
        let (directory, executable) = fixture("cat >/dev/null");
        let mut active = start_fixture(&executable).unwrap();
        let missing = directory.path().join("absent-engine");
        let error = ControllerEngine::start_with_reaper(
            &missing,
            &StdClock::new(),
            Reaper::refuse_preparation,
        )
        .unwrap_err();
        let preserved = active.try_wait().unwrap().is_none();
        active.request_shutdown();
        let stopped = wait_for_exit(&mut active);
        assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
        assert!(preserved);
        assert!(stopped.success());
    }

    #[test]
    fn child_spawn_failure_closes_the_prepared_reaper() {
        let directory = tempdir().unwrap();
        let (reaper, observed) = Reaper::observe_exit().unwrap();
        let error = ControllerEngine::start_with_reaper(
            &directory.path().join("absent-engine"),
            &StdClock::new(),
            || Ok(reaper),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn invalid_readiness_closes_the_prepared_reaper() {
        let (_directory, executable) = fixture_without_ready("printf 'MRREADY?\\n'\nexit 0");
        let (reaper, observed) = Reaper::observe_exit().unwrap();
        let error =
            ControllerEngine::start_with_reaper(&executable, &StdClock::new(), || Ok(reaper))
                .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn drop_returns_while_the_prepared_reaper_waits_for_audio_drain() {
        let (_directory, executable) = fixture(
            "cat >/dev/null\ni=0\nwhile [ ! -f \"$0.release\" ] && [ \"$i\" -lt 500 ]; do\n  i=$((i+1))\n  sleep 0.01\ndone",
        );
        let (reaper, observed) = Reaper::observe_exit().unwrap();
        let engine =
            ControllerEngine::start_with_reaper(&executable, &StdClock::new(), || Ok(reaper))
                .unwrap();
        let (finished, returned) = std::sync::mpsc::channel();
        let dropping = thread::spawn(move || {
            drop(engine);
            finished.send(()).unwrap();
        });
        let owner_returned = returned.recv_timeout(Duration::from_secs(1));
        // Release the fixture before asserting, including if Drop incorrectly waited.
        fs::write(executable.with_extension("sh.release"), b"release").unwrap();
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        dropping.join().unwrap();
        assert!(
            owner_returned.is_ok(),
            "controller Drop waited for audio drain"
        );
    }

    #[test]
    fn malformed_fixture_is_rejected_and_reaped() {
        let (_directory, executable) = fixture_without_ready("printf 'MRREADY?\\n'\nexit 0");
        let Err(error) = start_fixture(&executable) else {
            panic!("malformed fixture must not start");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn child_exiting_before_ready_is_rejected() {
        let (_directory, executable) = fixture_without_ready("exit 0");
        let Err(error) = start_fixture(&executable) else {
            panic!("early-exit fixture must not start");
        };
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn drop_closes_lease_without_waiting_for_the_child() {
        let (_directory, executable) = fixture("cat >/dev/null");
        let engine = start_fixture(&executable).expect("fixture should start");
        let marker = marker_path(&executable);
        let started = Instant::now();
        drop(engine);
        assert!(started.elapsed() < Duration::from_secs(1));
        wait_for_marker(&marker);
    }

    /// Linux refuses to execute a file another process still holds open for writing
    /// (`ETXTBSY`). A child forked by a parallel test can inherit this test's write
    /// handle until it execs, so starting the fixture just written retries that one
    /// error a bounded number of times.
    fn start_fixture(executable: &Path) -> io::Result<ControllerEngine> {
        for _ in 0..100 {
            match ControllerEngine::start(executable, &StdClock::new()) {
                Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy => {
                    thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
        ControllerEngine::start(executable, &StdClock::new())
    }

    fn fixture(body: &str) -> (TempDir, PathBuf) {
        fixture_script(&format!("printf 'MRREADY\\n'\n{body}"))
    }

    fn fixture_without_ready(body: &str) -> (TempDir, PathBuf) {
        fixture_script(body)
    }

    fn fixture_script(body: &str) -> (TempDir, PathBuf) {
        let directory = tempdir().expect("temporary fixture directory");
        let executable = directory.path().join("engine.sh");
        let script =
            format!("#!/bin/sh\nset -eu\ntrap 'printf done > \"$0.marker\"' EXIT\n{body}\n");
        fs::write(&executable, script).expect("fixture script");
        let mut permissions = fs::metadata(&executable)
            .expect("fixture metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).expect("fixture permissions");
        (directory, executable)
    }

    fn marker_path(executable: &Path) -> PathBuf {
        PathBuf::from(format!("{}.marker", executable.display()))
    }

    fn wait_for_exit(engine: &mut ControllerEngine) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = engine.try_wait().expect("fixture status") {
                return status;
            }
            assert!(Instant::now() < deadline, "fixture did not exit");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_marker(marker: &Path) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.is_file() {
            assert!(
                Instant::now() < deadline,
                "fixture did not observe lease closure"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
