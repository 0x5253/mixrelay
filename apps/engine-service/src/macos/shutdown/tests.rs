//! Shutdown ownership and failure propagation.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
#[ignore = "subprocess fixture; launched only by the ownership-loss test"]
fn lease_owner_fixture() {
    let Some(path) = std::env::var_os("MIXRELAY_BACKGROUND_STOP_TEST_LEASE") else {
        return;
    };
    let lease = mixrelay_macos_transport::app_lifecycle::ProcessLease::acquire(path)
        .expect("isolated test lease");
    lease.record("ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[tokio::test]
async fn killed_controller_releases_os_lease_and_runs_driver_stop() {
    let directory = mixrelay_test_support::FixtureDir::new("background-stop").unwrap();
    let path = directory.join("controller.owner");
    std::fs::File::create(&path).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "macos::shutdown::tests::lease_owner_fixture",
        ])
        .env("MIXRELAY_BACKGROUND_STOP_TEST_LEASE", &path)
        .spawn()
        .unwrap();
    let mut ready = false;
    for _ in 0..200 {
        if std::fs::read_to_string(&path).unwrap() == "ready" {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Always reap the fixture before an assertion can unwind the test.
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        panic!("lease fixture did not become ready");
    }
    let live = tokio::time::timeout(Duration::from_millis(150), wait_for_controller(&path)).await;
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert!(
        live.is_err(),
        "a live background controller must retain audio"
    );
    tokio::time::timeout(Duration::from_secs(1), wait_for_controller(&path))
        .await
        .expect("SIGKILL releases the process-owned lock");
    let stopped = AtomicBool::new(false);
    finish(Ok(()), Ok(()), async {
        stopped.store(true, Ordering::Release);
        Ok(())
    })
    .await
    .unwrap();
    assert!(stopped.load(Ordering::Acquire));
}

#[tokio::test]
async fn retirement_failure_still_stops_native_audio() {
    let stopped = AtomicBool::new(false);
    let result = finish(Ok(()), Err("recording finalization failed".into()), async {
        stopped.store(true, Ordering::Release);
        Ok(())
    })
    .await;
    assert!(stopped.load(Ordering::Acquire));
    assert_eq!(
        result,
        Err("Native retirement: recording finalization failed".into())
    );
}

#[tokio::test]
async fn native_stop_failure_is_not_a_successful_engine_exit() {
    assert_eq!(
        finish(Ok(()), Ok(()), async { Err("devices did not stop".into()) }).await,
        Err("Driver shutdown failed: devices did not stop".into())
    );
    let errors = finish(Err("recording failed".into()), Ok(()), async {
        Err("state was not saved".into())
    })
    .await
    .unwrap_err();
    assert!(errors.contains("recording failed"));
    assert!(errors.contains("state was not saved"));
}

#[tokio::test]
async fn all_shutdown_boundary_failures_are_preserved() {
    let runtime = Err("audio worker failed".into());
    let retired = Err("native retirement failed".into());
    let errors = finish(runtime, retired, async { Err("driver stop failed".into()) })
        .await
        .unwrap_err();
    assert!(errors.contains("audio worker failed"));
    assert!(errors.contains("native retirement failed"));
    assert!(errors.contains("driver stop failed"));
}

#[test]
fn only_a_clean_run_is_recorded_as_stopped() {
    assert_eq!(exit_record(&Ok(())), "stopped");
    assert_eq!(
        exit_record(&Err(
            "control: listener failed\naudio_join: worker panicked".into()
        )),
        "failed\ncontrol: listener failed\naudio_join: worker panicked"
    );
}

#[tokio::test]
async fn controller_release_stops_the_service_without_a_signal() {
    let stop = first_stop(std::future::pending(), std::future::ready(()));
    assert!(stop.await.is_ok());
}

#[tokio::test]
async fn a_failed_signal_observer_stops_the_service_with_its_error() {
    let stop = first_stop(
        std::future::ready(Err(io::Error::other("signal stream closed"))),
        std::future::pending(),
    );
    assert_eq!(stop.await.unwrap_err().to_string(), "signal stream closed");
}
