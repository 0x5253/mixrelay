//! One-operation authorization and mutually authenticated XPC for the installed remover.
//!
//! The UI remains a normal user process. Only the PKG-installed root helper runs
//! the existing fixed-path removal transaction. It accepts no path, command or UID.

use std::{
    ffi::{CString, c_char, c_int, c_void},
    fs,
    os::unix::fs::MetadataExt,
    path::Path,
    process::Command,
    ptr::NonNull,
    sync::Mutex,
    time::{Duration, Instant},
};

use crate::{
    REMOVAL_LAUNCHD_PATH, ReleasePayloadVerifier, ServiceAccount, SigningTeam, UNINSTALLER_PATH,
    native::launchctl, require_root,
};
use anyhow::{Context, Result, ensure};

/// Fixed system Mach service used only for authenticated Driver removal.
pub const SERVICE: &str = "io.mixrelay.driver.removal";
/// Root-owned CLI executable embedded in the separately installed remover.
pub const HELPER: &str = crate::REMOVAL_HELPER_PATH;

mod executable;
use executable::RemovalExecutable;

struct RemovalListener {
    executable: Mutex<Option<RemovalExecutable>>,
    signing_team: SigningTeam,
    session: Mutex<crate::removal_session::RemovalSession>,
}

unsafe extern "C" {
    fn MRRemovalRightRegister() -> c_int;
    fn MRRemovalRightRemove() -> c_int;
    fn MRRemovalAuthorize(prompt: *const c_char, output: *mut *mut c_void) -> c_int;
    fn MRRemovalAuthorizationFree(reference: *mut c_void);
    fn MRRemovalRequest(
        reference: *mut c_void,
        team: *const c_char,
        delete_settings: bool,
    ) -> c_int;
    fn MRRemovalServer(
        team: *const c_char,
        context: *mut c_void,
        remove: extern "C" fn(*mut c_void, bool) -> c_int,
        finish: extern "C" fn(*mut c_void),
        idle: extern "C" fn(*mut c_void),
        transition: extern "C" fn(*mut c_void, u32, usize, i32) -> u32,
    ) -> c_int;
}

/// An authorization for the named removal right, destroyed when its request finishes.
pub struct RemovalAuthorization(NonNull<c_void>);

// SAFETY: Authorization Services references are thread-safe. Ownership moves to
// one worker; the reference is not cloned or accessed concurrently.
unsafe impl Send for RemovalAuthorization {}

impl RemovalAuthorization {
    /// Runs system authentication. Cancellation returns no authorization and changes no Driver state.
    /// # Errors
    /// Returns an OS authorization failure or invalid prompt.
    pub fn request(prompt: &str) -> Result<Option<Self>> {
        let prompt = CString::new(prompt)?;
        let mut reference = std::ptr::null_mut();
        // SAFETY: both pointers are valid for the synchronous call; C creates an owned reference.
        let status = unsafe { MRRemovalAuthorize(prompt.as_ptr(), &raw mut reference) };
        if status == -60006 {
            return Ok(None);
        }
        ensure!(status == 0, "removal authorization failed: {status}");
        Ok(Some(Self(
            NonNull::new(reference).context("missing removal authorization")?,
        )))
    }

    /// Sends only the authorization and settings Bool over mutually signature-checked XPC.
    /// # Errors
    /// Returns helper rejection, IPC failure, timeout, or incomplete helper retirement.
    pub fn remove(self, team: &SigningTeam, delete_settings: bool) -> Result<()> {
        let team = CString::new(team.as_str())?;
        // SAFETY: the reference remains owned until this bounded call returns; C copies its blob.
        let result = unsafe { MRRemovalRequest(self.0.as_ptr(), team.as_ptr(), delete_settings) };
        ensure!(
            result == 0,
            "authenticated removal helper did not report committed success"
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while helper_is_registered()? {
            ensure!(
                Instant::now() < deadline,
                "removal helper registration remained after commit"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        for target in crate::OwnedTarget::all() {
            ensure!(
                fs::symlink_metadata(target.path())
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
                "owned target remains after removal"
            );
        }
        Ok(())
    }
}

impl Drop for RemovalAuthorization {
    fn drop(&mut self) {
        // SAFETY: uniquely owned native reference, freed exactly once.
        unsafe {
            MRRemovalAuthorizationFree(self.0.as_ptr());
        }
    }
}

/// Validates the exact fixed root helper configuration; it cannot supply arbitrary argv.
/// # Errors
/// Returns malformed, oversized or out-of-contract configuration data.
pub fn verify_removal_launchd(path: &Path, team: &SigningTeam) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.len() <= 16 * 1024,
        "invalid removal helper plist"
    );
    let actual: plist::Value = plist::from_bytes(&fs::read(path)?)?;
    let expected: plist::Value = plist::from_bytes(removal_launchd_template(team).as_bytes())?;
    ensure!(
        actual == expected,
        "removal helper plist has an unexpected operation or destination"
    );
    Ok(())
}

/// Returns the closed helper plist for one validated signing team.
#[must_use]
pub fn removal_launchd_template(team: &SigningTeam) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{SERVICE}</string>
<key>ProgramArguments</key><array><string>{HELPER}</string><string>removal-helper</string><string>{}</string></array>
<key>MachServices</key><dict><key>{SERVICE}</key><true/></dict>
<key>AssociatedBundleIdentifiers</key><array><string>io.mixrelay.driver.uninstaller</string></array>
</dict></plist>
"#,
        team.as_str()
    )
}

/// Installs the operation's authorization policy and bootstraps its idle on-demand job.
/// # Errors
/// Returns root/trust, policy registration or launchd failure.
pub fn register_helper(team: &SigningTeam) -> Result<()> {
    require_root()?;
    verify_installed_remover(team)?;
    register_verified_helper()
}

/// Restores the previous remover after ownership and preflight identity verification.
pub(crate) fn restore_helper(
    verifier: &ReleasePayloadVerifier,
    layout: &crate::FilesystemLayout,
    snapshot: &crate::OwnedInstallationSnapshot,
    team: &SigningTeam,
) -> Result<()> {
    require_root()?;
    verifier.verify_restored(
        layout,
        &[
            crate::OwnedTarget::Uninstaller,
            crate::OwnedTarget::RemovalLaunchd,
        ],
        snapshot,
    )?;
    verify_remaining_remover(team)?;
    register_verified_helper()
}

fn register_verified_helper() -> Result<()> {
    // SAFETY: no caller pointers; this installs only the fixed right in the native shim.
    let status = unsafe { MRRemovalRightRegister() };
    ensure!(
        status == 0,
        "could not register removal operation authorization: {status}"
    );
    if !helper_is_registered()? {
        launchctl::bootstrap(&launchctl::REMOVAL_HELPER)?;
    }
    Ok(())
}

/// Stops the fixed helper job before an installer replaces its executable.
/// # Errors
/// Returns root or launchd failure.
pub fn unregister_helper() -> Result<()> {
    require_root()?;
    if helper_is_registered()? {
        launchctl::bootout(&launchctl::REMOVAL_HELPER)?;
    }
    Ok(())
}

/// Removes the named operation policy after a completed removal.
/// # Errors
/// Returns root or authorization database failure.
pub fn remove_right() -> Result<()> {
    require_root()?;
    // SAFETY: fixed authorization right only, no caller-provided identifier.
    let status = unsafe { MRRemovalRightRemove() };
    ensure!(
        status == 0,
        "could not retire removal authorization: {status}"
    );
    Ok(())
}

fn helper_is_registered() -> Result<bool> {
    Ok(matches!(
        launchctl::inspect(&launchctl::REMOVAL_HELPER)?,
        launchctl::RegistrationState::Loaded { .. }
    ))
}

/// Verifies the root-owned installed app, embedded CLI and exact launchd definition.
/// # Errors
/// Returns any ownership, symlink, code-signing or fixed-path violation.
pub fn verify_installed_remover(team: &SigningTeam) -> Result<()> {
    let bundle = UNINSTALLER_PATH;
    validate_root_tree(Path::new(bundle))?;
    validate_root_file(Path::new(REMOVAL_LAUNCHD_PATH), 0o644)?;
    crate::native::validate_destination_ancestors(Path::new("/"))?;
    ReleasePayloadVerifier::new(team.clone(), verification_account()?)
        .verify_uninstaller_bundle(bundle)?;
    verify_removal_launchd(Path::new(REMOVAL_LAUNCHD_PATH), team)
}

/// Verifies remaining remover components independently for an idempotent removal.
/// # Errors
/// Returns unsafe ownership, present code signatures or launchd definitions.
pub fn verify_remaining_remover(team: &SigningTeam) -> Result<()> {
    crate::native::validate_destination_ancestors(Path::new("/"))?;
    for path in [UNINSTALLER_PATH, REMOVAL_LAUNCHD_PATH] {
        match fs::symlink_metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("inspect remaining remover component"),
        }
        if path == UNINSTALLER_PATH {
            validate_root_tree(Path::new(path))?;
            ReleasePayloadVerifier::new(team.clone(), verification_account()?)
                .verify_uninstaller_ownership(Path::new(path))?;
        } else {
            validate_root_file(Path::new(path), 0o644)?;
            verify_removal_launchd(Path::new(path), team)?;
        }
    }
    Ok(())
}

fn validate_root_file(path: &Path, mode: u32) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.nlink() == 1
            && metadata.mode() & 0o777 == mode,
        "untrusted removal helper file"
    );
    Ok(())
}

fn validate_root_tree(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        !metadata.file_type().is_symlink() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "untrusted removal helper bundle entry"
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            validate_root_tree(&entry?.path())?;
        }
    } else {
        ensure!(
            metadata.is_file() && metadata.nlink() == 1,
            "invalid removal bundle entry"
        );
    }
    Ok(())
}

/// Runs the fixed system helper. A valid named authorization is checked in C before callback.
/// # Errors
/// Returns an invalid helper location, private executable handoff or listener failure.
pub fn run_helper_listener(team: &SigningTeam) -> Result<()> {
    use std::os::unix::process::CommandExt;

    require_root()?;
    verify_remaining_remover(team)?;
    if std::env::current_exe()? == Path::new(HELPER) {
        // XPC verifies the sender's executable when delivering its reply. The
        // installed remover is removed by this transaction, so run this same
        // signed helper from a private copy until the reply has been accepted.
        // exec preserves the launchd job's PID and Mach-service ownership.
        let stage = RemovalExecutable::create_private_copy(team)?;
        let error = Command::new(&stage.executable)
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .args(["removal-helper", team.as_str()])
            .exec();
        return Err(error).context("could not hand off removal listener to private executable");
    }
    let stage = RemovalExecutable::from_current_process(team)?;
    let mut listener = Box::new(RemovalListener {
        executable: Mutex::new(Some(stage)),
        signing_team: team.clone(),
        session: Mutex::new(crate::removal_session::RemovalSession::default()),
    });
    let team = CString::new(team.as_str())?;
    // SAFETY: the boxed owner remains live for the synchronous server call. Native
    // callbacks run on its serial queue, and dispatch_main does not return. A startup
    // failure returns without publishing callbacks.
    let status = unsafe {
        MRRemovalServer(
            team.as_ptr(),
            (&raw mut *listener).cast(),
            remove_callback,
            finish_callback,
            idle_callback,
            transition_callback,
        )
    };
    listener.cleanup_executable()?;
    anyhow::bail!("removal listener could not start: {status}")
}

impl RemovalListener {
    fn cleanup_executable(&self) -> Result<()> {
        let stage = self
            .executable
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(stage) = stage {
            stage.remove_private_copy()?;
        }
        Ok(())
    }

    fn finish(&self) {
        if let Err(error) = self.cleanup_executable() {
            tracing::error!(error = %error, "private removal executable cleanup failed");
            std::process::exit(1);
        }
        if let Err(error) = unregister_helper() {
            tracing::error!(error = %error, "removal helper retirement failed");
        }
        std::process::exit(0);
    }

    fn idle(&self) {
        if let Err(error) = self.cleanup_executable() {
            tracing::error!(error = %error, "idle removal executable cleanup failed");
            std::process::exit(1);
        }
        std::process::exit(0);
    }

    fn remove(&self, delete_settings: bool) -> c_int {
        // The callback is an unwind boundary; a panic must never cross into XPC.
        let result = std::panic::catch_unwind(|| self.execute(delete_settings));
        match result {
            Ok(Ok(())) => 0,
            failure => {
                tracing::error!(failure = ?failure, "authorized removal failed");
                if [UNINSTALLER_PATH, REMOVAL_LAUNCHD_PATH].iter().all(|path| {
                    fs::symlink_metadata(path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                }) {
                    2
                } else {
                    1
                }
            }
        }
    }
}

extern "C" fn finish_callback(context: *mut c_void) {
    // SAFETY: MRRemovalServer passes the boxed owner while its call remains active.
    if let Some(listener) = unsafe { context.cast::<RemovalListener>().as_ref() } {
        listener.finish();
    }
}

extern "C" fn idle_callback(context: *mut c_void) {
    // SAFETY: MRRemovalServer passes the boxed owner while its call remains active.
    if let Some(listener) = unsafe { context.cast::<RemovalListener>().as_ref() } {
        listener.idle();
    }
}

extern "C" fn remove_callback(context: *mut c_void, delete_settings: bool) -> c_int {
    // SAFETY: MRRemovalServer passes the boxed owner while its call remains active.
    unsafe { context.cast::<RemovalListener>().as_ref() }
        .map_or(1, |listener| listener.remove(delete_settings))
}

extern "C" fn transition_callback(
    context: *mut c_void,
    event: u32,
    peer: usize,
    status: i32,
) -> u32 {
    use crate::removal_session::{Action, Event};
    // SAFETY: MRRemovalServer passes the boxed owner while its call remains active.
    let Some(listener) = (unsafe { context.cast::<RemovalListener>().as_ref() }) else {
        return Action::Ignore as u32;
    };
    let event = match event {
        1 => Event::Admission,
        2 => Event::Transaction(status),
        3 => Event::Acknowledged,
        4 => Event::Disconnected,
        5 => Event::Abandoned,
        6 => Event::IdleDeadline,
        7 => Event::Terminated,
        _ => return Action::Ignore as u32,
    };
    listener
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .event(event, peer) as u32
}

fn verification_account() -> Result<ServiceAccount> {
    // Bundle verification does not use an account-bound Driver plist. The actual
    // transaction resolves the original account from that verified installed plist.
    Ok(ServiceAccount::new(
        "mixrelay-preflight".into(),
        "staff".into(),
        "/Users/mixrelay-preflight".into(),
    )?)
}

impl RemovalListener {
    fn execute(&self, delete_settings: bool) -> Result<()> {
        require_root()?;
        // The listener's validated startup identity survives a user deleting the
        // installed plist or bundle. Missing files must not invalidate authorization.
        let team = &self.signing_team;
        verify_remaining_remover(team)?;
        // The listener's verified private executable also remains available to the
        // normal-user cleanup and app-session subprocesses after the remover is deleted.
        let mut command = Command::new(std::env::current_exe()?);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .args(["remove", team.as_str(), "--reload-coreaudio"]);
        if delete_settings {
            command.arg("--delete-user-data");
        }
        ensure!(
            command.status()?.success(),
            "fixed removal transaction failed"
        );
        Ok(())
    }
}
