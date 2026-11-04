//! Bounded reads and atomic replacement of desktop preference documents.
use mixrelay_desktop_core::recording::PreferencesRepository;
use std::{
    fs,
    io::{Read as _, Write as _},
    path::PathBuf,
};

/// One explicitly selected preference document; construction performs no file I/O.
#[derive(Debug)]
pub(crate) struct AtomicPreferences {
    path: PathBuf,
    limit: usize,
}
impl AtomicPreferences {
    pub(crate) fn new(path: PathBuf, limit: usize) -> Self {
        Self { path, limit }
    }
}
impl PreferencesRepository for AtomicPreferences {
    fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
        match fs::File::open(&self.path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((self.limit as u64).saturating_add(1))
                    .read_to_end(&mut bytes)?;
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn replace(&self, bytes: &[u8]) -> std::io::Result<()> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("preferences need a parent directory"))?;
        fs::create_dir_all(directory)?;
        let mut staged = tempfile::NamedTempFile::new_in(directory)?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(&self.path).map_err(|error| error.error)?;
        if let Err(error) = fs::File::open(directory).and_then(|directory| directory.sync_all()) {
            tracing::warn!(%error, "desktop preferences directory sync failed after commit");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_document_reads_as_none_until_the_first_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let preferences = AtomicPreferences::new(directory.path().join("new/prefs.json"), 64);
        assert!(preferences.read().unwrap().is_none());
        preferences.replace(b"{\"first\":true}").unwrap();
        preferences.replace(b"{}").unwrap();
        assert_eq!(preferences.read().unwrap().unwrap(), b"{}");
    }

    #[test]
    fn an_oversized_document_reads_one_byte_past_the_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefs.json");
        fs::write(&path, [b'x'; 32]).unwrap();
        let read = AtomicPreferences::new(path, 8).read().unwrap().unwrap();
        assert_eq!(read.len(), 9);
    }
}
