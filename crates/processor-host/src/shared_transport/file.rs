//! Private file mappings and the exclusive isolated-worker lease.
use super::{
    ControlMailbox, Mapping, ParentAudio, SharedHeader, TransportRegion, WorkerAudio,
    WorkerControl, initialize_private, shared_length,
};
use crate::AudioLayout;
use memmap2::MmapRaw;
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::Arc,
};

/// Creates transport at an explicit fixture path.
#[cfg(test)]
pub(crate) fn create(
    path: &Path,
    layout: AudioLayout,
    instance_id: crate::instance_id::InstanceId,
) -> Result<(ParentAudio, ControlMailbox), String> {
    create_at(path, layout, instance_id, None)
}

pub(crate) fn create_owned(
    paths: &crate::worker_paths::WorkerPaths,
    layout: AudioLayout,
    instance_id: crate::instance_id::InstanceId,
) -> Result<(ParentAudio, ControlMailbox), String> {
    create_at(
        paths.shared(),
        layout,
        instance_id,
        Some(&paths.directory()),
    )
}

fn create_at(
    path: &Path,
    layout: AudioLayout,
    instance_id: crate::instance_id::InstanceId,
    directory: Option<&Arc<tempfile::TempDir>>,
) -> Result<(ParentAudio, ControlMailbox), String> {
    let length = shared_length(layout).map_err(|error| error.to_string())?;
    let header = SharedHeader::new(layout, instance_id)?;
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    let result = (|| {
        file.set_len(
            u64::try_from(length)
                .map_err(|_| "processor transport length exceeds u64".to_owned())?,
        )
        .map_err(|error| error.to_string())?;
        let region = FileRegion::open(&file, None, directory.cloned())?;
        // SAFETY: the new private file has not been published to either protocol peer.
        unsafe { initialize_private(&region, header) }.map_err(|error| error.to_string())?;
        region.raw.flush().map_err(|error| error.to_string())?;
        Ok((
            ParentAudio::from_mapping(open_mapping(&file, None, directory.cloned())?)?,
            ControlMailbox::from_mapping(open_mapping(&file, None, directory.cloned())?),
        ))
    })();
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Called once by the isolated worker entry point before starting any worker thread.
/// The parent creates exactly one child for this private transport path.
pub(crate) fn worker(path: &Path) -> Result<(WorkerAudio, WorkerControl), String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.try_lock()
        .map_err(|error| format!("could not claim processor transport: {error}"))?;
    let lease = Arc::new(file);
    Ok((
        WorkerAudio::from_mapping(open_mapping(&lease, Some(lease.clone()), None)?),
        WorkerControl::from_mapping(open_mapping(&lease, Some(lease.clone()), None)?),
    ))
}

struct FileRegion {
    raw: MmapRaw,
    // Drop the address-space view before releasing its share of the worker lease.
    _worker_lease: Option<Arc<File>>,
    // The final view closes before its private namespace can be removed.
    _directory: Option<Arc<tempfile::TempDir>>,
}
impl FileRegion {
    fn open(
        file: &File,
        worker_lease: Option<Arc<File>>,
        directory: Option<Arc<tempfile::TempDir>>,
    ) -> Result<Self, String> {
        Ok(Self {
            raw: MmapRaw::map_raw(file).map_err(|error| error.to_string())?,
            _worker_lease: worker_lease,
            _directory: directory,
        })
    }
}
// SAFETY: MmapRaw owns initialized, stable mapped bytes. Protocol peers retain independent
// views, and the worker lease outlives each view. Only the protocol's owners mutate regions.
unsafe impl TransportRegion for FileRegion {
    fn byte_length(&self) -> usize {
        self.raw.len()
    }
    fn as_ptr(&self) -> *const u8 {
        self.raw.as_ptr()
    }
    fn as_mut_ptr(&self) -> *mut u8 {
        self.raw.as_mut_ptr()
    }
}
fn open_mapping(
    file: &File,
    lease: Option<Arc<File>>,
    directory: Option<Arc<tempfile::TempDir>>,
) -> Result<Mapping, String> {
    Mapping::admit(
        Arc::new(FileRegion::open(file, lease, directory)?),
        Arc::new(FileWake::new()),
    )
    .map_err(|error| error.to_string())
}

struct FileWake {
    #[cfg(target_os = "macos")]
    futex: Option<&'static mixrelay_realtime::SharedFutex>,
}
impl FileWake {
    fn new() -> Self {
        Self {
            // Resolve native symbols during preparation, before a callback can signal.
            #[cfg(target_os = "macos")]
            futex: mixrelay_realtime::shared_futex(),
        }
    }
}
impl super::WorkerWake for FileWake {
    fn signal(&self, word: &std::sync::atomic::AtomicU64) {
        #[cfg(target_os = "macos")]
        if let Some(futex) = self.futex {
            futex.wake(word);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = word;
    }
}
