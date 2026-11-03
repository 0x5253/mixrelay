//! Build and verify the actual Rust HAL bundle, without a parallel shell build.
//! This command never installs a driver or changes the running audio service.
use std::{env, error::Error, ffi::OsString, path::PathBuf, process::ExitCode};

mod build;
mod command;
#[cfg(target_os = "macos")]
mod probe;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("HAL bundle build failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    if !cfg!(target_os = "macos") {
        return Err("the HAL bundle must be built on macOS".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()?;
    let output = output_directory(&env::args_os().skip(1).collect::<Vec<_>>(), &root)?;
    let identity = env::var_os("MIXRELAY_CODESIGN_IDENTITY").unwrap_or_else(|| "-".into());
    build::bundle(&root, &output, &identity)
}

fn output_directory(
    arguments: &[OsString],
    root: &std::path::Path,
) -> Result<PathBuf, Box<dyn Error>> {
    match arguments {
        [] => Ok(root.join("target/macos-hal")),
        [flag, directory] if flag == "--output" && !directory.is_empty() => Ok(directory.into()),
        _ => Err("usage: cargo run -p mixrelay-hal-bundle -- [--output DIRECTORY]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_defaults_to_workspace_artifacts_not_temporary_storage() {
        assert_eq!(
            output_directory(&[], std::path::Path::new("/workspace")).unwrap(),
            PathBuf::from("/workspace/target/macos-hal")
        );
    }

    #[test]
    fn output_accepts_paths_with_spaces_without_shell_evaluation() {
        assert_eq!(
            output_directory(
                &["--output".into(), "/workspace/build dir".into()],
                std::path::Path::new("/workspace")
            )
            .unwrap(),
            PathBuf::from("/workspace/build dir")
        );
        assert!(
            output_directory(&["--output".into()], std::path::Path::new("/workspace")).is_err()
        );
        assert!(
            output_directory(
                &["--output".into(), "".into()],
                std::path::Path::new("/workspace")
            )
            .is_err()
        );
    }
}
