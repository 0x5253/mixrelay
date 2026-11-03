use std::{fs, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result, anyhow, ensure};
use mixrelay_macos_installer::{
    OwnedTarget, ReleasePayloadVerifier, SigningTeam, resolve_console_account,
};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertSecondButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSButton, NSButtonType, NSControlStateValueOff, NSControlStateValueOn,
};
use objc2_foundation::{NSBundle, NSLocale, NSPoint, NSRect, NSSize, NSString};

use super::validate_bundle_layout;

#[path = "localization.rs"]
mod localization;
use localization::{Text, UiLocale};

#[derive(Clone, Copy)]
struct NativeUi {
    main: MainThreadMarker,
    locale: UiLocale,
}

impl NativeUi {
    fn text(self, text: Text) -> &'static str {
        self.locale.text(text)
    }
}

#[derive(Clone, Copy, Debug)]
enum Diagnostic {
    DownloadAgain,
    Authorization,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DownloadAgain => "The signed removal bundle could not be verified.",
            Self::Authorization => "Could not authorize or confirm the fixed removal operation.",
        })
    }
}

impl std::error::Error for Diagnostic {}

fn error_text(error: &anyhow::Error) -> Text {
    error
        .downcast_ref::<Diagnostic>()
        .map_or(Text::Retry, |diagnostic| match diagnostic {
            Diagnostic::DownloadAgain => Text::DownloadAgain,
            Diagnostic::Authorization => Text::Authorization,
        })
}

/// Presents removal consent and diagnostics on the process's main thread.
pub(super) fn run() -> Result<()> {
    let main =
        MainThreadMarker::new().ok_or_else(|| anyhow!("AppKit requires the main thread."))?;
    let languages: Vec<_> = NSLocale::preferredLanguages()
        .iter()
        .map(|language| language.to_string())
        .collect();
    let ui = NativeUi {
        main,
        locale: UiLocale::from_preferences(languages.iter().map(String::as_str)),
    };
    let application = NSApplication::sharedApplication(main);
    if application.activationPolicy() != NSApplicationActivationPolicy::Regular {
        application.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    }
    ensure!(
        application.activationPolicy() == NSApplicationActivationPolicy::Regular,
        "Could not activate the uninstaller."
    );
    application.finishLaunching();
    application.activate();
    let result = remove_driver(ui);
    if let Err(error) = &result {
        tracing::error!("Driver removal failed: {error:#}");
        message(
            ui,
            ui.text(Text::FailedTitle),
            ui.text(error_text(error)),
            NSAlertStyle::Warning,
        );
    }
    result
}

fn remove_driver(ui: NativeUi) -> Result<()> {
    let Some(delete_settings) = confirm_removal(ui) else {
        return Ok(());
    };
    if !driver_is_installed()? {
        let account = resolve_console_account()?;
        mixrelay_macos_installer::cleanup_user_data(
            account.account().home(),
            delete_settings,
            false,
        )?;
        message(
            ui,
            ui.text(Text::NotInstalledTitle),
            ui.text(Text::NothingToRemove),
            NSAlertStyle::Informational,
        );
        return Ok(());
    }
    let (bundle, helper, team) = bundled_helper().context(Diagnostic::DownloadAgain)?;
    // Signature policy needs only an OS-resolved identity; the authorized CLI
    // separately resolves the installed service account under its root boundary.
    let account = resolve_console_account()?;
    let verifier =
        ReleasePayloadVerifier::new(SigningTeam::new(team.clone())?, account.account().clone());
    verifier
        .verify_uninstaller_bundle(&bundle)
        .context("The uninstaller's signature could not be verified. Download it again.")
        .context(Diagnostic::DownloadAgain)?;
    verifier
        .verify_installer_executable(&helper)
        .context("The bundled removal tool's signature could not be verified.")
        .context(Diagnostic::DownloadAgain)?;
    let Some(authorization) =
        mixrelay_macos_installer::removal::RemovalAuthorization::request(ui.text(Text::Remove))
            .context(Diagnostic::Authorization)?
    else {
        return Ok(());
    };
    let team = SigningTeam::new(team)?;
    // Authentication succeeded; later failures concern removal/confirmation,
    // and must not tell the user that administrator authentication failed.
    perform_removal(ui, authorization, team, delete_settings)?;
    message(
        ui,
        ui.text(Text::RemovedTitle),
        ui.text(if delete_settings {
            Text::SettingsDeleted
        } else {
            Text::SettingsPreserved
        }),
        NSAlertStyle::Informational,
    );
    Ok(())
}

fn driver_is_installed() -> Result<bool> {
    for path in OwnedTarget::all().map(OwnedTarget::path) {
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Could not inspect the Driver installation."),
        }
    }
    Ok(false)
}

fn confirm_removal(ui: NativeUi) -> Option<bool> {
    let alert = NSAlert::new(ui.main);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(ui.text(Text::ConfirmTitle)));
    alert.setInformativeText(&NSString::from_str(ui.text(Text::ConfirmDetail)));
    let cancel = alert.addButtonWithTitle(&NSString::from_str(ui.text(Text::Cancel)));
    cancel.setKeyEquivalent(&NSString::from_str("\u{1b}"));
    alert.addButtonWithTitle(&NSString::from_str(ui.text(Text::Remove)));
    let checkbox = NSButton::initWithFrame(
        NSButton::alloc(ui.main),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 24.0)),
    );
    checkbox.setButtonType(NSButtonType::Switch);
    checkbox.setTitle(&NSString::from_str(ui.text(Text::DeleteSettings)));
    checkbox.setState(NSControlStateValueOff);
    checkbox.sizeToFit();
    alert.setAccessoryView(Some(&checkbox));
    (alert.runModal() == NSAlertSecondButtonReturn)
        .then(|| checkbox.state() == NSControlStateValueOn)
}

fn bundled_helper() -> Result<(PathBuf, PathBuf, String)> {
    let bundle = NSBundle::mainBundle();
    let bundle_path = fs::canonicalize(bundle.bundlePath().to_string())
        .context("Could not locate the uninstaller bundle.")?;
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    let helper = fs::canonicalize(bundle_path.join("Contents/Helpers/mixrelay-macos-installer"))
        .context("The bundled removal tool is missing.")?;
    validate_bundle_layout(&bundle_path, &executable, &helper)?;
    let team = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("MixRelaySigningTeam"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .ok_or_else(|| anyhow!("The uninstaller's publisher identity is missing."))?
        .to_string();
    Ok((bundle_path, helper, team))
}

fn message(ui: NativeUi, title: &str, detail: &str, style: NSAlertStyle) {
    let alert = NSAlert::new(ui.main);
    alert.setAlertStyle(style);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(detail));
    alert.addButtonWithTitle(&NSString::from_str(ui.text(Text::Ok)));
    alert.runModal();
}

fn perform_removal(
    ui: NativeUi,
    authorization: mixrelay_macos_installer::removal::RemovalAuthorization,
    team: SigningTeam,
    delete_settings: bool,
) -> Result<()> {
    use objc2_app_kit::{
        NSBackingStoreType, NSProgressIndicator, NSTextField, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSRunLoop};
    // SAFETY: main-thread marker, finite frame, and no delegated window lifetime.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(ui.main),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(480.0, 100.0)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // AppKit must not release the Rust-owned window when it closes.
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(&NSString::from_str(ui.text(Text::Remove)));
    let label = NSTextField::labelWithString(&NSString::from_str(ui.text(Text::Working)), ui.main);
    label.setFrame(NSRect::new(
        NSPoint::new(20.0, 60.0),
        NSSize::new(440.0, 24.0),
    ));
    let progress = NSProgressIndicator::initWithFrame(
        NSProgressIndicator::alloc(ui.main),
        NSRect::new(NSPoint::new(20.0, 25.0), NSSize::new(440.0, 16.0)),
    );
    progress.setIndeterminate(true);
    // SAFETY: nil sender; progress indicator belongs to the main thread.
    unsafe {
        progress.startAnimation(None);
    }
    let content = window.contentView().context("missing progress content")?;
    content.addSubview(&label);
    content.addSubview(&progress);
    window.center();
    window.makeKeyAndOrderFront(None);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(authorization.remove(&team, delete_settings));
    });
    let result = loop {
        match receiver.try_recv() {
            Ok(result) => break result,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                break Err(anyhow!("removal worker ended without a result"));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
            }
        }
    };
    // SAFETY: nil sender; the main thread still owns the indicator.
    unsafe {
        progress.stopAnimation(None);
    }
    window.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_failures_are_logged_but_not_used_as_user_diagnostics() {
        let error = anyhow!("internal command failure with a private path");
        let detail = UiLocale::French.text(error_text(&error));
        assert_eq!(detail, UiLocale::French.text(Text::Retry));
        assert!(!detail.contains("private path"));
        assert!(detail.contains("support@rawsector.com"));
    }

    #[test]
    fn typed_failure_context_selects_localized_recovery() {
        let error = anyhow!("technical signature details").context(Diagnostic::DownloadAgain);
        assert_eq!(
            UiLocale::German.text(error_text(&error)),
            UiLocale::German.text(Text::DownloadAgain)
        );
        let error = anyhow!("OS authorization details").context(Diagnostic::Authorization);
        assert_eq!(
            UiLocale::Korean.text(error_text(&error)),
            UiLocale::Korean.text(Text::Authorization)
        );
    }
}
