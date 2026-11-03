//! Fixed `MixRelay` app actions performed in its normal user's GUI session.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, ensure};
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration};
use objc2_foundation::{NSArray, NSDate, NSRunLoop, NSString, NSURL};

use mixrelay_macos_installer::process_exit::{ProcessIdentity, wait_for_exit};

use mixrelay_macos_installer::APP_PATH;
use mixrelay_macos_installer::session::SessionApp;

/// The bundle identifier shared by the editor and runtime roles.
const APP_IDENTIFIER: &str = "io.mixrelay.desktop";

pub(super) fn quit(app: SessionApp) -> Result<bool> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "app actions require a normal user"
    );
    let applications = matching_applications(app)?;
    // Identify each process by its kernel audit token first; the exit is judged from
    // that identity, not from `isTerminated`, which a command-line process may never
    // see change. The PID-based checks below are bound to the identity by re-checking
    // it afterwards, and an app that exits meanwhile is skipped.
    let role = app.quit_action();
    let mut targets = Vec::new();
    for application in &applications {
        let pid = application.processIdentifier();
        let Some(process) = ProcessIdentity::capture(pid)
            .with_context(|| format!("{role}: could not identify pid {pid}"))?
        else {
            continue;
        };
        let owner = std::process::Command::new("/bin/ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "uid=", "-o", "args="])
            .output()?;
        let observed = String::from_utf8(owner.stdout)?;
        let (uid, command) = observed.trim().split_once(' ').unwrap_or_default();
        let path = application.bundleURL().and_then(|url| url.path());
        if process
            .has_exited()
            .with_context(|| format!("{role}: pid {pid}"))?
        {
            continue;
        }
        ensure!(
            owner.status.success() && uid == unsafe { libc::geteuid() }.to_string(),
            "{role}: app belongs to a different user (pid {pid})"
        );
        ensure!(
            app.matches_command(command.trim())
                && path.is_some_and(|path| path.to_string() == APP_PATH),
            "{role}: unexpected app process (pid {pid})"
        );
        targets.push((application, process));
    }
    let was_running = !targets.is_empty();
    for (application, process) in &targets {
        // `terminate` reports false for an app that exited after it was identified.
        ensure!(
            application.terminate()
                || process
                    .has_exited()
                    .with_context(|| format!("{role}: pid {}", process.pid()))?,
            "{role} refused Quit (pid {})",
            process.pid()
        );
    }
    let processes: Vec<_> = targets.iter().map(|(_, process)| *process).collect();
    let pump = |interval: Duration| {
        NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(
            interval.as_secs_f64(),
        ));
    };
    // The runtime stops audio, finalizes recordings and waits for the Driver before it
    // exits, so a normal Quit gets most of the budget.
    let started = Instant::now();
    let remaining = wait_for_exit(&processes, started + QUIT_GRACE, pump)
        .with_context(|| format!("{role}: waiting for Quit"))?;
    if remaining.is_empty() {
        return Ok(was_running);
    }
    // An app that did not act on the Quit event (for example a busy main thread) gets
    // SIGTERM through its audit token; the runtime routes it through the same
    // acknowledged shutdown, and the editor owns no audio state.
    for process in &remaining {
        process
            .terminate()
            .with_context(|| format!("{role}: SIGTERM to pid {}", process.pid()))?;
    }
    let remaining = wait_for_exit(&remaining, started + QUIT_BUDGET, pump)
        .with_context(|| format!("{role}: waiting for SIGTERM"))?;
    ensure!(
        remaining.is_empty(),
        "{role} did not complete Quit (pid {})",
        remaining
            .iter()
            .map(|process| process.pid().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(was_running)
}

/// Time a normal Quit may take before the installer escalates to `SIGTERM`.
const QUIT_GRACE: Duration = Duration::from_secs(100);
/// The whole Quit budget, including the `SIGTERM` escalation.
const QUIT_BUDGET: Duration = Duration::from_secs(130);

pub(super) fn restore(app: SessionApp) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "app actions require a normal user"
    );
    // Launch Services gives the app its own privacy responsibility. A direct
    // installer child inherits Installer's TCC identity even after reparenting.
    let configuration = NSWorkspaceOpenConfiguration::configuration();
    configuration.setCreatesNewApplicationInstance(true);
    configuration.setAllowsRunningApplicationSubstitution(false);
    configuration.setActivates(app == SessionApp::Graph);
    if app == SessionApp::UnifiedRuntime {
        configuration.setArguments(&NSArray::from_retained_slice(&[NSString::from_str(
            "--driver-runtime",
        )]));
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(APP_PATH));
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(
        &url,
        &configuration,
        None,
    );
    // Accepting a launch request is not proof that the requested role launched.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let applications = matching_applications(app)?;
        for application in &applications {
            if application.isTerminated() || !application.isFinishedLaunching() {
                continue;
            }
            let path = application.bundleURL().and_then(|url| url.path());
            if path.is_none_or(|path| path.to_string() != APP_PATH) {
                continue;
            }
            let owner = std::process::Command::new("/bin/ps")
                .args([
                    "-p",
                    &application.processIdentifier().to_string(),
                    "-o",
                    "uid=",
                ])
                .output()?;
            if owner.status.success()
                && String::from_utf8(owner.stdout)?.trim() == unsafe { libc::geteuid() }.to_string()
                && !application.isTerminated()
            {
                return Ok(());
            }
        }
        ensure!(Instant::now() < deadline, "app relaunch was not observed");
        NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
    }
}

fn matching_applications(
    app: SessionApp,
) -> Result<Vec<objc2::rc::Retained<NSRunningApplication>>> {
    let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(APP_IDENTIFIER),
    );
    let mut matching = Vec::new();
    for application in &applications {
        if application.isTerminated() {
            continue;
        }
        let output = std::process::Command::new("/bin/ps")
            .args([
                "-ww",
                "-p",
                &application.processIdentifier().to_string(),
                "-o",
                "args=",
            ])
            .output()?;
        if output.status.success() && app.matches_command(String::from_utf8(output.stdout)?.trim())
        {
            matching.push(application.clone());
        }
    }
    Ok(matching)
}
