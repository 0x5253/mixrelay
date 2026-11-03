//! Root-only install/remove entry point; never elevates itself or signs payloads.

#[cfg(target_os = "macos")]
#[path = "installer/app_session.rs"]
mod app_session;
#[cfg(target_os = "macos")]
use mixrelay_macos_installer::session;

fn main() {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    if let Err(error) = run() {
        tracing::error!("MixRelay installation failed: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "macos"))]
fn run() -> anyhow::Result<()> {
    anyhow::bail!("mixrelay-macos-installer is available only on macOS")
}

#[cfg(target_os = "macos")]
fn run() -> anyhow::Result<()> {
    use std::ffi::OsString;

    use mixrelay_macos_installer::{
        SigningTeam, TransactionRequest, cleanup_user_data, require_root, resolve_install_account,
        resolve_removal_account,
    };

    let arguments = std::env::args_os().skip(1).collect::<Vec<OsString>>();
    if arguments.as_slice() == [OsString::from("--help")] {
        print_usage();
        return Ok(());
    }
    if arguments.as_slice() == [OsString::from("check-coreaudio")] {
        return mixrelay_macos_installer::check_coreaudio_response().map_err(Into::into);
    }
    if arguments.as_slice() == [OsString::from("probe-hal-absence")] {
        // A separate read-only process makes blocking CoreAudio calls cancellable.
        let loaded = mixrelay_macos_installer::hal_is_loaded()?;
        std::process::exit(if loaded { 2 } else { 0 });
    }
    if arguments.as_slice() == [OsString::from("prepare-graph-logs")] {
        return create_graph_logs();
    }
    for app in session::SessionApp::ALL {
        if arguments.as_slice() == [OsString::from(app.quit_action())] {
            return objc2::rc::autoreleasepool(|_| {
                println!("{}", app_session::quit(app)?);
                Ok(())
            });
        }
        if arguments.as_slice() == [OsString::from(app.restore_action())] {
            return objc2::rc::autoreleasepool(|_| app_session::restore(app));
        }
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "cleanup-user-data")
    {
        let (home, delete_settings, wait_for_commit) = parse_cleanup(&arguments)?;
        return cleanup_user_data(&home, delete_settings, wait_for_commit);
    }
    if let [command, team] = arguments.as_slice()
        && command == "removal-helper"
    {
        require_root()?;
        return mixrelay_macos_installer::removal::run_helper_listener(&SigningTeam::new(
            required_text(team, "Team ID")?,
        )?);
    }
    let (request, team, delete_user_data) = parse_request(&arguments)?;
    if matches!(request, TransactionRequest::Remove) {
        refuse_removing_own_executable(&std::env::current_exe()?)?;
    }
    require_root()?;
    let team = SigningTeam::new(team)?;
    let account = match &request {
        TransactionRequest::Install(_) => resolve_install_account()?,
        TransactionRequest::Remove => resolve_removal_account()?,
    };
    run_transaction(&request, team, account, delete_user_data)
}

/// Removal moves every installed target, including the uninstaller's own helper. The
/// transaction keeps launching this executable (`CoreAudio` probes, user cleanup), so it must
/// run from the private copy the uninstaller makes, never from inside a target it removes.
#[cfg(target_os = "macos")]
fn refuse_removing_own_executable(executable: &std::path::Path) -> anyhow::Result<()> {
    let product = std::path::Path::new(mixrelay_macos_installer::APP_PATH)
        .parent()
        .unwrap_or_else(|| std::path::Path::new(mixrelay_macos_installer::APP_PATH));
    let inside = mixrelay_macos_installer::OwnedTarget::all()
        .iter()
        .map(|target| std::path::Path::new(target.path()))
        .chain(std::iter::once(product))
        .any(|target| executable.starts_with(target));
    anyhow::ensure!(
        !inside,
        "removal must run from Uninstall MixRelay, which runs a private copy of this helper; {} would be removed while it runs",
        executable.display()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_transaction(
    request: &mixrelay_macos_installer::TransactionRequest,
    team: mixrelay_macos_installer::SigningTeam,
    account: mixrelay_macos_installer::ResolvedServiceAccount,
    delete_user_data: bool,
) -> anyhow::Result<()> {
    use mixrelay_macos_installer::{
        CoordinatedBackend, FilesystemLayout, NativeServiceLifecycle, TransactionRequest, execute,
    };
    use session::{SessionApp, dependencies, run_with_apps_closed};
    let session_account = account.clone();
    let layout = FilesystemLayout::new("/")?;
    let installed_layout = layout.clone();
    let recovery_verifier = mixrelay_macos_installer::ReleasePayloadVerifier::new(
        team.clone(),
        account.account().clone(),
    );
    let install = matches!(request, TransactionRequest::Install(_));
    if let TransactionRequest::Install(sources) = request {
        // Reject invalid update inputs before interrupting either application.
        // The transaction verifies its private copies again before commit.
        recovery_verifier.verify_sources(sources)?;
    }
    // Capture before Quit so failures before transaction preflight can reopen only
    // the previous owned code, including versions with different entitlements.
    let previous = recovery_verifier.snapshot_owned(&installed_layout)?;
    let mut running = Vec::new();
    // The complete product owns the editor as well as the audio runtime.
    // Acknowledged Quit saves state before replacement; relaunch follows commit.
    for app in SessionApp::ALL {
        if app_running(&account, app)? {
            running.push(app);
        }
    }
    let service = NativeServiceLifecycle::new(team, account)
        .with_coreaudio_reload_approval(true)
        .with_saved_settings_removal(delete_user_data);
    let mut backend = CoordinatedBackend::new(layout, service);
    let may_relaunch_apps = std::cell::Cell::new(true);
    let outcome = run_with_apps_closed(
        &running,
        install,
        |action| user_session_action(&session_account, action),
        || {
            // Running apps hold exclusive lifecycle leases. Initialize/rebind
            // only after their acknowledged normal-user Quit has completed.
            mixrelay_macos_installer::initialize_lifecycle_storage(&session_account)?;
            if install {
                mixrelay_macos_installer::prepare_install_directories()?;
                prepare_graph_logs(&session_account)?;
            }
            execute(&mut backend, request).map_err(|error| {
                may_relaunch_apps.set(!matches!(
                    &error,
                    mixrelay_macos_installer::TransactionError::MutationRecovery {
                        recovery: mixrelay_macos_installer::RecoveryStatus::Failed { .. },
                        ..
                    } | mixrelay_macos_installer::TransactionError::Cleanup(_)
                ));
                error.into()
            })
        },
        // A failed repair leaves the previous, possibly partial, installation: reopen
        // the closed apps when what they need is present and everything present verifies.
        |closed| {
            may_relaunch_apps.get()
                && recovery_verifier
                    .verify_restored(&installed_layout, &dependencies(closed), &previous)
                    .is_ok()
        },
    )?;
    tracing::info!(operation = ?outcome.operation(), state = ?outcome.state(),
        "MixRelay installation transaction and app session restoration completed");
    Ok(())
}

#[cfg(target_os = "macos")]
fn app_running(
    account: &mixrelay_macos_installer::ResolvedServiceAccount,
    app: session::SessionApp,
) -> anyhow::Result<bool> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axww", "-o", "uid=,args="])
        .output()?;
    anyhow::ensure!(output.status.success(), "could not inspect app owners");
    let expected_uid = account.uid().to_string();
    Ok(String::from_utf8(output.stdout)?.lines().any(|line| {
        let line = line.trim();
        line.find(char::is_whitespace).is_some_and(|split| {
            line[..split] == expected_uid && app.matches_command(line[split..].trim())
        })
    }))
}

#[cfg(target_os = "macos")]
fn user_session_action(
    account: &mixrelay_macos_installer::ResolvedServiceAccount,
    action: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        action == "prepare-graph-logs"
            || session::SessionApp::ALL
                .iter()
                .any(|app| action == app.quit_action() || action == app.restore_action()),
        "invalid session action"
    );
    let output = std::process::Command::new("/bin/launchctl")
        .args([
            "asuser",
            &account.uid().to_string(),
            "/usr/bin/sudo",
            "-n",
            "-u",
            &format!("#{}", account.uid()),
            "--",
        ])
        .arg(std::env::current_exe()?)
        .arg(action)
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("HOME", account.account().home())
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "app session action failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

/// Creates the graph app log directory as the installed user.
#[cfg(target_os = "macos")]
fn create_graph_logs() -> anyhow::Result<()> {
    // SAFETY: geteuid only queries this process identity.
    anyhow::ensure!(
        unsafe { libc::geteuid() } != 0,
        "app logs must be created as the service user"
    );
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("missing service home"))?;
    std::fs::create_dir_all(std::path::PathBuf::from(home).join("Library/Logs/MixRelay"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn prepare_graph_logs(
    account: &mixrelay_macos_installer::ResolvedServiceAccount,
) -> anyhow::Result<()> {
    user_session_action(account, "prepare-graph-logs").map(|_| ())
}

#[cfg(any(target_os = "macos", test))]
fn parse_cleanup(
    arguments: &[std::ffi::OsString],
) -> anyhow::Result<(std::path::PathBuf, bool, bool)> {
    let (home, settings, wait_for_commit) = match arguments {
        [command, home, settings] if command == "cleanup-user-data" => (home, settings, false),
        [command, home, settings, wait]
            if command == "cleanup-user-data" && wait == "--wait-for-commit" =>
        {
            (home, settings, true)
        }
        _ => anyhow::bail!("invalid cleanup arguments"),
    };
    let delete_settings = match settings.to_str() {
        Some("--delete-user-data") => true,
        Some("--preserve-user-data") => false,
        _ => anyhow::bail!("invalid cleanup settings selection"),
    };
    Ok((
        std::path::PathBuf::from(home),
        delete_settings,
        wait_for_commit,
    ))
}

#[cfg(any(target_os = "macos", test))]
fn parse_request(
    arguments: &[std::ffi::OsString],
) -> anyhow::Result<(mixrelay_macos_installer::TransactionRequest, String, bool)> {
    use mixrelay_macos_installer::{InstallSources, TransactionRequest};
    use std::path::PathBuf;

    let Some(command) = arguments.first().and_then(|value| value.to_str()) else {
        anyhow::bail!("expected install or remove");
    };
    let request_and_team = match command {
        "install" if arguments.len() == 11 && arguments[10] == "--reload-coreaudio" => {
            let team = required_text(&arguments[1], "Team ID")?;
            let path = |index: usize| PathBuf::from(&arguments[index]);
            let sources = InstallSources::new(
                path(2),
                path(3),
                (path(4), path(5)),
                (path(6), path(7)),
                (path(8), path(9)),
            );
            (TransactionRequest::install(sources), team)
        }
        "remove"
            if (arguments.len() == 3 && arguments[2] == "--reload-coreaudio")
                || (arguments.len() == 4
                    && arguments[2] == "--reload-coreaudio"
                    && arguments[3] == "--delete-user-data") =>
        {
            let team = required_text(&arguments[1], "Team ID")?;
            (TransactionRequest::remove(), team)
        }
        _ => {
            anyhow::bail!(
                "install/remove require explicit --reload-coreaudio approval and valid arguments"
            );
        }
    };
    let delete_user_data = command == "remove" && arguments.len() == 4;
    Ok((request_and_team.0, request_and_team.1, delete_user_data))
}

#[cfg(any(target_os = "macos", test))]
fn required_text(argument: &std::ffi::OsStr, label: &str) -> anyhow::Result<String> {
    argument
        .to_str()
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("{label} must be nonempty UTF-8"))
}

#[cfg(target_os = "macos")]
fn print_usage() {
    eprintln!(
        "Usage:\n  mixrelay-macos-installer install TEAM_ID MixRelay.app HAL.driver DRIVER PLIST_TEMPLATE UNINSTALLER.app REMOVAL_PLIST ENGINE ENGINE_PLIST --reload-coreaudio\n  mixrelay-macos-installer remove TEAM_ID --reload-coreaudio [--delete-user-data]\n\nInstall/remove require an already-authorized root process and explicit approval to reload CoreAudio, interrupting active audio without rebooting the Mac. Removal optionally clears Driver device settings as the verified installation account. Updates preserve all user data. The authorized root command drops to the installed user for app Quit and relaunch. It never elevates itself, signs, or notarizes code."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn removal_refuses_to_run_from_inside_a_target_it_removes() {
        for inside in [
            "/Applications/MixRelay/Uninstall MixRelay.app/Contents/Helpers/mixrelay-macos-installer",
            "/Applications/MixRelay/MixRelay.app/Contents/MacOS/mixrelay-desktop",
        ] {
            assert!(
                refuse_removing_own_executable(std::path::Path::new(inside)).is_err(),
                "{inside}"
            );
        }
        assert!(
            refuse_removing_own_executable(std::path::Path::new(
                "/private/var/run/mixrelay-removal/mixrelay-macos-installer"
            ))
            .is_ok()
        );
    }
    use mixrelay_macos_installer::TransactionRequest;
    use std::ffi::OsString;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn cleanup_accepts_only_the_declared_optional_commit_wait() {
        for selected in ["--delete-user-data", "--preserve-user-data"] {
            let direct = arguments(&["cleanup-user-data", "/Users/test", selected]);
            assert!(!parse_cleanup(&direct).expect("direct cleanup").2);
            let mut waiting = direct;
            waiting.push(OsString::from("--wait-for-commit"));
            assert!(parse_cleanup(&waiting).expect("prepared cleanup").2);
            waiting.push(OsString::from("extra"));
            assert!(parse_cleanup(&waiting).is_err());
        }
        assert!(
            parse_cleanup(&arguments(&[
                "cleanup-user-data",
                "/Users/test",
                "--wait-for-commit",
                "--delete-user-data"
            ]))
            .is_err()
        );
    }

    #[test]
    fn install_and_remove_require_explicit_reload_approval_in_the_declared_position() {
        let install = [
            "install",
            "TEAM",
            "MixRelay.app",
            "HAL.driver",
            "Driver",
            "template.plist",
            "Uninstaller.app",
            "removal.plist",
            "Engine",
            "engine.plist",
        ];
        assert!(parse_request(&arguments(&install)).is_err());
        let mut approved = arguments(&install);
        approved.push(OsString::from("--reload-coreaudio"));
        assert!(matches!(
            parse_request(&approved).unwrap().0,
            TransactionRequest::Install(_)
        ));
        for invalid in [
            vec!["remove", "TEAM"],
            vec!["remove", "TEAM", "--delete-user-data"],
            vec!["remove", "TEAM", "--delete-user-data", "--reload-coreaudio"],
        ] {
            assert!(parse_request(&arguments(&invalid)).is_err());
        }
        assert!(
            !parse_request(&arguments(&["remove", "TEAM", "--reload-coreaudio"]))
                .unwrap()
                .2
        );
        assert!(
            parse_request(&arguments(&[
                "remove",
                "TEAM",
                "--reload-coreaudio",
                "--delete-user-data"
            ]))
            .unwrap()
            .2
        );
    }
}
