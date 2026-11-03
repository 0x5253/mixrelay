#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a failed setup fails the test"
)]
//! First-party storage must not depend on machine-wide temporary directories.
//! This is a source contract, not a claim about third-party tools' internals.
use std::{fs, path::Path};

fn forbidden_storage(source: &str) -> bool {
    // Assemble the patterns so this regression test is itself scan-safe.
    [
        ["/", "tmp/"].concat(),
        ["/var/", "tmp/"].concat(),
        ["temp_", "dir("].concat(),
        ["os.", "tmpdir("].concat(),
        ["mktemp", " -d)"].concat(),
        ["${", "TMPDIR"].concat(),
    ]
    .iter()
    .any(|pattern| source.contains(pattern))
}

fn inspect(directory: &Path, failures: &mut Vec<String>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some("target" | "node_modules" | "dist" | "gen" | "storybook-static")
            ) {
                inspect(&path, failures);
            }
        } else if path.file_name().and_then(|name| name.to_str()) == Some("Dockerfile")
            || matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("rs" | "c" | "h" | "cpp" | "m" | "mm" | "sh" | "mjs")
            )
        {
            let source = fs::read_to_string(&path).unwrap();
            if forbidden_storage(&source) {
                failures.push(path.display().to_string());
            }
        }
    }
}

#[test]
fn first_party_storage_has_no_system_temporary_directory_fallback() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut failures = Vec::new();
    for directory in ["apps", "crates", "native", "scripts", "infra"] {
        inspect(&workspace.join(directory), &mut failures);
    }
    assert!(
        failures.is_empty(),
        "forbidden temporary storage: {failures:#?}"
    );
}

#[test]
fn guard_rejects_implicit_and_explicit_temporary_storage() {
    assert!(forbidden_storage(&["/", "tmp/file"].concat()));
    assert!(forbidden_storage(&["std::env::temp_", "dir()"].concat()));
    assert!(forbidden_storage(&["$(mktemp", " -d)"].concat()));
    assert!(!forbidden_storage("target/test-artifacts/fixture"));
    assert!(!forbidden_storage("Library/Caches/MixRelay/processors"));
}
