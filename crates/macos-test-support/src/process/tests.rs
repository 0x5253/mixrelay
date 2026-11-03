use std::{
    io::{self, Write},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use super::{Deadline, OUTPUT_LIMIT, POLL_INTERVAL, run, run_until};

const CHILD_NAME: &str = "process::tests::owned_child_fixture";
const CHILD_MODE: &str = "MIXRELAY_RUNNER_FIXTURE_MODE";
/// Bounds only a stalled run. No outcome depends on how quickly a child starts or prints.
const HANG_GUARD: Duration = Duration::from_mins(1);

/// Expires once the child printed a complete line for every identity prefix, so the
/// timeout under test always ends a run whose processes are all known to exist.
/// The elapsed guard only ends a stalled run, which then fails on its missing identity.
struct Ready {
    prefixes: &'static [&'static str],
    guard: Instant,
}

impl Ready {
    fn after(prefixes: &'static [&'static str]) -> impl FnOnce() -> Self {
        move || Self {
            prefixes,
            guard: Instant::now() + HANG_GUARD,
        }
    }
}

impl Deadline for Ready {
    fn expired(&mut self, stdout: &[u8]) -> bool {
        let stdout = String::from_utf8_lossy(stdout);
        self.prefixes
            .iter()
            .all(|prefix| identity(&stdout, prefix).is_some())
            || Instant::now() >= self.guard
    }
    fn wait(&self) -> Duration {
        POLL_INTERVAL
    }
}

fn child(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command.args(["--exact", CHILD_NAME, "--nocapture", "--test-threads=1"]);
    command.env(CHILD_MODE, mode);
    command
}

/// This Cargo test is also the owned subprocess fixture. Normal execution does
/// nothing; subprocess modes are selected only by this module's explicit env.
#[test]
fn owned_child_fixture() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    println!("fixture_pid={}", std::process::id());
    io::stdout().flush().expect("flush fixture identity");
    match mode.as_str() {
        "success" => {
            println!("fixture stdout");
            eprintln!("fixture stderr");
        }
        "failure" => std::process::exit(9),
        "hang" => loop {
            thread::sleep(Duration::from_secs(1));
        },
        "descendant" => {
            let mut descendant = child("hang").spawn().expect("owned descendant");
            println!("descendant_pid={}", descendant.id());
            io::stdout().flush().expect("flush descendant identity");
            let _ = descendant.wait();
        }
        // Twice the diagnostic limit, then a normal exit: never unbounded.
        "noisy" => {
            for _ in 0..=2 * OUTPUT_LIMIT / 8192 {
                println!("{}", "x".repeat(8192));
            }
        }
        other => panic!("unknown fixture mode: {other}"),
    }
}

#[test]
fn runner_captures_success_and_failure() {
    let success = run(&mut child("success"), HANG_GUARD).expect("success runner");
    assert!(success.status.success());
    assert!(String::from_utf8_lossy(&success.stdout).contains("fixture stdout"));
    assert!(String::from_utf8_lossy(&success.stderr).contains("fixture stderr"));
    let failure = run(&mut child("failure"), HANG_GUARD).expect("failure runner");
    assert_eq!(failure.status.code(), Some(9));
    assert_reaped(pid(
        &String::from_utf8_lossy(&failure.stdout),
        "fixture_pid=",
    ));
}

#[test]
fn elapsed_deadline_ends_a_run_that_never_exits() {
    // A zero timeout expires at the first decision, before any output is required.
    let error = run(&mut child("hang"), Duration::ZERO).expect_err("hanging child must fail");
    assert!(error.contains("deadline exceeded"), "{error}");
}

#[test]
fn runner_times_out_and_reaps_the_child() {
    let error = run_until(&mut child("hang"), Ready::after(&["fixture_pid="]))
        .expect_err("hanging child must fail");
    assert!(error.contains("deadline exceeded"), "{error}");
    assert_reaped(pid(&error, "fixture_pid="));
}

#[test]
fn runner_timeout_terminates_only_its_owned_process_group() {
    let error = run_until(
        &mut child("descendant"),
        Ready::after(&["fixture_pid=", "descendant_pid="]),
    )
    .expect_err("hanging descendant must fail");
    assert!(error.contains("deadline exceeded"), "{error}");
    assert_reaped(pid(&error, "fixture_pid="));
    assert_reaped(pid(&error, "descendant_pid="));
}

#[test]
fn runner_limits_diagnostics_without_hanging() {
    // No deadline applies. The child cannot finish writing twice the limit before the runner
    // has read past the limit, so only a broken limit lets the child exit and the run succeed.
    let error =
        run(&mut child("noisy"), Duration::MAX).expect_err("output beyond the limit must fail");
    assert!(
        error.contains("output exceeded 1 MiB"),
        "missing output limit failure"
    );
    assert_reaped(pid(&error, "fixture_pid="));
}

#[test]
fn runner_reports_launch_failures() {
    let error = run(
        &mut Command::new("/nonexistent-mixrelay-owned-test-fixture"),
        HANG_GUARD,
    )
    .expect_err("missing binary must fail");
    assert!(error.contains("cannot launch"));
}

/// The process ID on the first complete line `prefix` starts.
fn identity(output: &str, prefix: &str) -> Option<i32> {
    output
        .split(prefix)
        .nth(1)
        .and_then(|suffix| suffix.split_once('\n'))
        .and_then(|(value, _)| value.parse().ok())
}

fn pid(output: &str, prefix: &str) -> i32 {
    identity(output, prefix).unwrap_or_else(|| panic!("no {prefix} line in: {output}"))
}

fn assert_reaped(pid: i32) {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        // SAFETY: signal zero probes existence without changing process state.
        if unsafe { libc::kill(pid, 0) } == -1 {
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
            return;
        }
        assert!(
            Instant::now() < deadline,
            "owned process {pid} survived cleanup"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
