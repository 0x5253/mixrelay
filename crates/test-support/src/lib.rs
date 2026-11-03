//! Test-only PCM measurements, checkout-local disposable artifacts and reply fixtures.
//! This crate is a dev-dependency only; production storage policy is separate.
pub mod frame_sequence;
pub mod reply_fixture;
pub mod rf64;
pub mod tone_continuity;

use std::{
    fs, io,
    ops::Deref,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// An exclusively created, checkout-local fixture directory, removed on Drop.
/// Ownership cannot be cloned or transferred to an arbitrary filesystem path.
pub struct FixtureDir(PathBuf);

#[derive(Clone, Copy)]
enum FixtureKind {
    Artifacts,
    Ipc,
}

impl FixtureDir {
    /// Creates an isolated fixture under `target/test-artifacts`.
    /// # Errors
    /// Rejects unsafe labels and reports directory creation failures.
    pub fn new(label: &str) -> io::Result<Self> {
        Self::create(label, FixtureKind::Artifacts)
    }

    /// Creates a checkout-local socket fixture under `target/.ipc`. Its fixed
    /// suffix keeps PID/counter growth out of the Unix socket path budget.
    /// # Errors
    /// Rejects invalid labels, checkouts too long for a 12-byte socket filename,
    /// and directory creation failures.
    pub fn ipc(label: &str) -> io::Result<Self> {
        Self::create(label, FixtureKind::Ipc)
    }

    fn create(label: &str, kind: FixtureKind) -> io::Result<Self> {
        let ipc = matches!(kind, FixtureKind::Ipc);
        if label.is_empty()
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture label must contain only ASCII letters, digits, or hyphens",
            ));
        }
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()?;
        let target = workspace.join("target");
        fs::create_dir_all(&target)?;
        let target = target.canonicalize()?;
        if !target.starts_with(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture Cargo target must remain inside its checkout",
            ));
        }
        let root = target.join(if ipc { ".ipc" } else { "test-artifacts" });
        fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        if !root.starts_with(&workspace) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture directory must remain inside its checkout",
            ));
        }
        // macOS allows 103 pathname bytes; leave a separator and 12-byte socket
        // name after label, hyphen and the eight-byte hexadecimal fixture suffix.
        if ipc && root.as_os_str().as_encoded_bytes().len() + label.len() + 23 > 103 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "checkout path exceeds the Unix socket fixture budget",
            ));
        }
        for _ in 0..128 {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let name = if ipc {
                let [a, b, c, d, ..] = sequence.to_le_bytes();
                let suffix =
                    std::process::id().wrapping_mul(0x9e37_79b9) ^ u32::from_le_bytes([a, b, c, d]);
                format!("{label}-{suffix:08x}")
            } else {
                format!("{label}-{}-{sequence}", std::process::id())
            };
            let path = root.join(name);
            match fs::create_dir(&path) {
                Ok(()) => {
                    let fixture = Self(path);
                    #[cfg(unix)]
                    if ipc {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700))?;
                    }
                    return Ok(fixture);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "fixture directory collision limit reached",
        ))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for FixtureDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.path()
    }
}

impl AsRef<Path> for FixtureDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            eprintln!(
                "owned test artifact cleanup failed for {}: {error}",
                self.0.display()
            );
            assert!(
                std::thread::panicking(),
                "owned test artifact cleanup failed: {error}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_fixture_has_a_fixed_short_name_private_permissions_and_owned_cleanup() {
        let path = {
            let directory = FixtureDir::ipc("ipc").unwrap();
            assert_eq!(directory.file_name().unwrap().as_encoded_bytes().len(), 12);
            assert!(
                directory
                    .join("engine.sock")
                    .as_os_str()
                    .as_encoded_bytes()
                    .len()
                    <= 103
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    directory.metadata().unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            directory.path().to_owned()
        };
        assert!(!path.exists());
    }

    #[test]
    fn owned_nested_artifacts_are_removed_on_scope_exit() {
        let path = {
            let directory = FixtureDir::new("nested-cleanup").unwrap();
            fs::create_dir(directory.join("nested")).unwrap();
            fs::write(directory.join("nested/example.json"), b"fixture").unwrap();
            assert!(
                directory.path().starts_with(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../..")
                        .canonicalize()
                        .unwrap()
                        .join("target/test-artifacts")
                )
            );
            directory.path().to_owned()
        };
        assert!(!path.exists());
    }

    #[test]
    fn same_label_is_isolated_and_panic_also_removes_owned_files() {
        let first = FixtureDir::new("same-label").unwrap();
        let second = FixtureDir::new("same-label").unwrap();
        assert_ne!(first.path(), second.path());
        let path = first.path().to_owned();
        assert!(
            std::panic::catch_unwind(move || {
                let _owned = first;
                panic!("fixture assertion");
            })
            .is_err()
        );
        assert!(!path.exists());
        assert!(second.path().is_dir());
    }

    #[test]
    fn labels_cannot_escape_the_owned_artifact_root() {
        for label in [
            "",
            "..",
            "../escape",
            "/absolute",
            "nested/name",
            "nested\\name",
        ] {
            assert_eq!(
                FixtureDir::new(label).err().unwrap().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
