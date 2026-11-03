//! Native Driver removal consent; the existing signed CLI owns the transaction.

#[cfg(target_os = "macos")]
#[path = "uninstaller/ui.rs"]
mod ui;

fn main() {
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    if let Err(error) = run() {
        tracing::error!("MixRelay removal failed: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
fn run() -> anyhow::Result<()> {
    objc2::rc::autoreleasepool(|_| ui::run())
}

#[cfg(not(target_os = "macos"))]
fn run() -> anyhow::Result<()> {
    anyhow::bail!("Uninstall MixRelay is available only on macOS")
}

#[cfg(any(target_os = "macos", test))]
fn validate_bundle_layout(
    bundle: &std::path::Path,
    executable: &std::path::Path,
    helper: &std::path::Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        bundle.is_absolute(),
        "The uninstaller must be an application bundle."
    );
    anyhow::ensure!(
        executable == bundle.join("Contents/MacOS/mixrelay-driver-uninstaller")
            && helper == bundle.join("Contents/Helpers/mixrelay-macos-installer"),
        "The uninstaller contains an unexpected executable path."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn only_fixed_bundle_executable_and_resource_helper_are_accepted() {
        let bundle = Path::new(mixrelay_macos_installer::UNINSTALLER_PATH);
        let executable = bundle.join("Contents/MacOS/mixrelay-driver-uninstaller");
        let helper = bundle.join("Contents/Helpers/mixrelay-macos-installer");
        assert!(validate_bundle_layout(bundle, &executable, &helper).is_ok());
        assert!(validate_bundle_layout(bundle, &executable, Path::new("/tmp/helper")).is_err());
        assert!(validate_bundle_layout(bundle, Path::new("/tmp/gui"), &helper).is_err());
        assert!(validate_bundle_layout(Path::new("relative.app"), &executable, &helper).is_err());
    }
}
