//! Read-only release gate; this is not an installer or a signing command.

#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use mixrelay_macos_installer::{
        InstallSources, ReleasePayloadVerifier, ServiceAccount, SigningTeam,
        validate_universal_binary,
    };
    use std::{env, ffi::OsString, path::PathBuf};

    let arguments: Vec<OsString> = env::args_os().skip(1).collect();
    let [root, team, user, group, home, installer] = arguments.as_slice() else {
        bail!(
            "usage: mixrelay-macos-payload-check PAYLOAD_ROOT TEAM_ID SERVICE_USER SERVICE_GROUP SERVICE_HOME INSTALLER_EXECUTABLE"
        );
    };
    let text = |value: &OsString| {
        value
            .to_str()
            .context("account arguments must be UTF-8")
            .map(str::to_owned)
    };
    let account = ServiceAccount::new(text(user)?, text(group)?, PathBuf::from(home))?;
    let verifier = ReleasePayloadVerifier::new(SigningTeam::new(text(team)?)?, account);
    let root = PathBuf::from(root);
    anyhow::ensure!(root.is_absolute(), "payload root must be absolute");
    let installer = PathBuf::from(installer);
    anyhow::ensure!(
        installer.is_absolute(),
        "installer executable must be absolute"
    );
    let sources = InstallSources::new(
        root.join(mixrelay_macos_installer::APP_PATH.trim_start_matches('/')),
        root.join("Library/Audio/Plug-Ins/HAL/MixRelayHAL.driver"),
        (
            root.join("Library/Application Support/MixRelay/bin/mixrelay-driver-service"),
            root.join("templates/io.mixrelay.audio.driver.plist.in"),
        ),
        (
            root.join(mixrelay_macos_installer::UNINSTALLER_PATH.trim_start_matches('/')),
            root.join("templates/io.mixrelay.driver.removal.plist"),
        ),
        (
            root.join("Library/Application Support/MixRelay/bin/mixrelay-service"),
            root.join("templates/io.mixrelay.audio.engine.plist.in"),
        ),
    );
    for binary in [
        sources.app().join("Contents/MacOS/mixrelay-desktop"),
        sources.hal().join("Contents/MacOS/MixRelayHAL"),
        sources.driver().to_path_buf(),
    ] {
        validate_universal_binary(&binary)
            .with_context(|| format!("invalid architecture: {}", binary.display()))?;
    }
    println!(
        "Universal 2 headers verified for app, HAL, and engine; checking publisher signatures"
    );
    verifier
        .verify_sources(&sources)
        .context("release payload rejected")?;
    verifier
        .verify_installer_executable(installer)
        .context("installer helper rejected")?;
    println!(
        "Release payload and installer-helper signatures and configuration verified; installation was not performed"
    );
    Ok(())
}

#[cfg(not(unix))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("macOS release payload verification requires macOS")
}
