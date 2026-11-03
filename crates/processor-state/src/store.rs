use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use crate::{
    MAX_STATE_BYTES, StateBlobSink, StateBlobSource, StateError, StateRef,
    payload::{hash_exact, hex_digest},
};

#[cfg(unix)]
#[path = "store/unix.rs"]
mod platform;
#[cfg(not(unix))]
compile_error!("processor state storage requires a supported native filesystem");
#[cfg(test)]
#[cfg(unix)]
#[path = "store/tests.rs"]
mod tests;

use platform::StoreDirectory;

const TEMP_CREATE_ATTEMPTS: usize = 128;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

/// A private directory containing immutable, SHA-256-addressed processor-state blobs.
#[derive(Clone, Debug)]
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    /// Creates a store handle for `root`; filesystem setup is deferred until an operation.
    ///
    /// # Arguments
    /// * `root` - The absolute private directory assigned to this store.
    ///
    /// # Returns
    /// A handle that creates or validates `root` on its first filesystem operation.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns the configured store directory.
    ///
    /// # Returns
    /// The path supplied to [`StateStore::new`].
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Interns bytes as an immutable blob and returns their validated content reference.
    ///
    /// # Arguments
    /// * `bytes` - The complete payload to store, bounded by [`MAX_STATE_BYTES`].
    ///
    /// # Returns
    /// The payload's validated SHA-256 reference.
    ///
    /// # Errors
    /// Returns [`StateError`] when the value exceeds [`MAX_STATE_BYTES`], its private store
    /// directory is unsafe, or filesystem operations fail.
    pub fn put(&self, bytes: &[u8]) -> Result<StateRef, StateError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(StateError::InvalidReference {
                reason: "state exceeds the maximum byte length",
            });
        }
        let byte_length = u64::try_from(bytes.len()).map_err(|_| StateError::InvalidReference {
            reason: "state length does not fit the supported size range",
        })?;
        let reference = StateRef::new(hex_digest(&Sha256::digest(bytes)), byte_length)?;
        match self.verify(&reference) {
            Ok(()) => return Ok(reference),
            Err(StateError::MissingBlob) => {}
            Err(error) => return Err(error),
        }
        self.store_payload(&mut io::Cursor::new(bytes), byte_length, Some(&reference))?;
        Ok(reference)
    }

    /// Reads and verifies a complete blob.
    ///
    /// # Arguments
    /// * `reference` - The validated content reference to resolve.
    ///
    /// # Returns
    /// The payload bytes after length and digest verification.
    ///
    /// # Errors
    /// Returns [`StateError`] for an invalid reference, missing or corrupt blob, unsafe store
    /// entry, or filesystem failure.
    pub fn read(&self, reference: &StateRef) -> Result<Vec<u8>, StateError> {
        reference.validate()?;
        let capacity =
            usize::try_from(reference.byte_length()).map_err(|_| StateError::InvalidReference {
                reason: "state length does not fit this platform",
            })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| StateError::Allocation)?;
        self.copy_payload(reference, &mut bytes)?;
        Ok(bytes)
    }

    /// Verifies a blob's file type, exact length, and SHA-256 using bounded memory.
    ///
    /// # Arguments
    /// * `reference` - The validated content reference to verify.
    ///
    /// # Returns
    /// Returns `Ok(())` only if the stored file matches the reference.
    ///
    /// # Errors
    /// Returns [`StateError`] for an invalid reference, missing or corrupt blob, unsafe store
    /// entry, or filesystem failure.
    pub fn verify(&self, reference: &StateRef) -> Result<(), StateError> {
        reference.validate()?;
        let directory = self.open_existing_directory()?;
        let mut file = open_blob(&directory, reference)?;
        hash_blob(&mut file, reference, None)
    }

    fn store_payload<R: Read + ?Sized>(
        &self,
        reader: &mut R,
        byte_length: u64,
        expected: Option<&StateRef>,
    ) -> Result<StateRef, StateError> {
        if byte_length > MAX_STATE_BYTES as u64 {
            return Err(StateError::InvalidReference {
                reason: "state exceeds the maximum byte length",
            });
        }
        if let Some(reference) = expected {
            reference.validate()?;
            if reference.byte_length() != byte_length {
                return Err(StateError::Integrity);
            }
        }

        let directory = self.open_directory(true)?;
        let mut temporary = TemporaryBlob::create(&directory)?;
        let sha256 = hash_exact(
            reader,
            byte_length,
            Some(temporary.file.as_mut().ok_or(StateError::Integrity)?),
            expected,
            expected.is_none(),
        )?;
        let reference = StateRef::new(sha256, byte_length)?;
        if expected.is_some_and(|expected| expected != &reference) {
            return Err(StateError::Integrity);
        }
        temporary.publish(&directory, &reference)?;
        Ok(reference)
    }

    fn copy_payload(&self, reference: &StateRef, writer: &mut dyn Write) -> Result<(), StateError> {
        reference.validate()?;
        let directory = self.open_existing_directory()?;
        let mut file = open_blob(&directory, reference)?;
        hash_blob(&mut file, reference, Some(writer))
    }

    fn open_directory(&self, create: bool) -> Result<StoreDirectory, StateError> {
        StoreDirectory::open(&self.root, create)
    }

    fn open_existing_directory(&self) -> Result<StoreDirectory, StateError> {
        self.open_directory(false).map_err(|error| match error {
            StateError::Io(io_error) if io_error.kind() == io::ErrorKind::NotFound => {
                StateError::MissingBlob
            }
            other => other,
        })
    }
}

impl StateBlobSource for StateStore {
    fn verify(&self, reference: &StateRef) -> Result<(), StateError> {
        // The inherent method of the same name, not this one.
        StateStore::verify(self, reference)
    }

    fn copy_verified(
        &self,
        reference: &StateRef,
        writer: &mut dyn Write,
    ) -> Result<(), StateError> {
        self.copy_payload(reference, writer)
    }
}

impl StateBlobSink for StateStore {
    fn store_verified(
        &self,
        reader: &mut dyn Read,
        reference: &StateRef,
    ) -> Result<(), StateError> {
        self.store_payload(reader, reference.byte_length(), Some(reference))
            .map(|_| ())
    }
}

fn open_blob(directory: &StoreDirectory, reference: &StateRef) -> Result<File, StateError> {
    let name = blob_name(reference);
    let file = directory.open_file(&name).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            StateError::MissingBlob
        } else {
            StateError::Io(error)
        }
    })?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !StoreDirectory::owns_blob(&metadata) {
        return Err(StateError::UnsafeStoreEntry);
    }
    if metadata.len() != reference.byte_length() {
        return Err(StateError::Integrity);
    }
    Ok(file)
}

fn blob_name(reference: &StateRef) -> String {
    format!("{}.state", reference.sha256())
}

fn hash_blob(
    file: &mut File,
    reference: &StateRef,
    writer: Option<&mut dyn Write>,
) -> Result<(), StateError> {
    let before = file.metadata()?;
    let digest = hash_exact(file, reference.byte_length(), writer, Some(reference), true)?;
    let after = file.metadata()?;
    if digest != reference.sha256() || !same_blob_metadata(&before, &after) {
        return Err(StateError::Integrity);
    }
    Ok(())
}

#[cfg(unix)]
fn same_blob_metadata(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mode() == after.mode()
        && before.uid() == after.uid()
        && before.nlink() == after.nlink()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

struct TemporaryBlob<'a> {
    directory: &'a StoreDirectory,
    name: String,
    file: Option<File>,
}

impl<'a> TemporaryBlob<'a> {
    fn create(directory: &'a StoreDirectory) -> Result<Self, StateError> {
        for _ in 0..TEMP_CREATE_ATTEMPTS {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let name = format!(".state-tmp-{}-{id}", std::process::id());
            match directory.create_temp(&name) {
                Ok(file) => {
                    return Ok(Self {
                        directory,
                        name,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(StateError::Io(error)),
            }
        }
        Err(StateError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique state staging file",
        )))
    }

    fn publish(
        &mut self,
        directory: &StoreDirectory,
        reference: &StateRef,
    ) -> Result<(), StateError> {
        let file = self.file.take().ok_or(StateError::Integrity)?;
        file.sync_all()?;
        drop(file);
        match directory.publish_no_replace(&self.name, &blob_name(reference)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                verify_in_directory(directory, reference)?;
            }
            Err(error) => return Err(StateError::Io(error)),
        }
        self.remove_temp();
        // Persist the directory after the no-replace rename completes.
        directory.sync()?;
        Ok(())
    }

    fn remove_temp(&self) {
        self.directory.remove_file(&self.name);
    }
}

impl Drop for TemporaryBlob<'_> {
    fn drop(&mut self) {
        self.file.take();
        self.remove_temp();
    }
}

fn verify_in_directory(directory: &StoreDirectory, reference: &StateRef) -> Result<(), StateError> {
    let mut file = open_blob(directory, reference)?;
    hash_blob(&mut file, reference, None)
}
