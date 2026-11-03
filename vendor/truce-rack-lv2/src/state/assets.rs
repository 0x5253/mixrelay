//! Synchronous LV2 mapPath/makePath scope. Only host-private, regular files are portable.

use super::{Asset, Result, error};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::{CStr, CString, c_char, c_void},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub(super) const MAX_ASSET_BYTES: usize = 40 * 1024 * 1024;
pub(super) const MAX_ASSETS: usize = 128;
pub(super) const MAP_PATH_URI: &[u8] = b"http://lv2plug.in/ns/ext/state#mapPath\0";
pub(super) const MAKE_PATH_URI: &[u8] = b"http://lv2plug.in/ns/ext/state#makePath\0";
pub(super) const FREE_PATH_URI: &[u8] = b"http://lv2plug.in/ns/ext/state#freePath\0";

#[repr(C)]
pub(super) struct MapPath {
    pub handle: *mut c_void,
    pub abstract_path: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    pub absolute_path: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
}

#[repr(C)]
pub(super) struct MakePath {
    pub handle: *mut c_void,
    pub path: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
}

#[repr(C)]
pub(super) struct FreePath {
    pub handle: *mut c_void,
    pub free_path: unsafe extern "C" fn(*mut c_void, *mut c_char),
}

struct Source {
    root: PathBuf,
    relative: PathBuf,
}

pub(super) struct SavePaths {
    roots: Vec<PathBuf>,
    sources: Vec<Source>,
    pub failure: Option<String>,
}

impl SavePaths {
    pub fn new(root: &Path, restored_root: Option<&Path>) -> Self {
        let mut roots = vec![root.to_path_buf()];
        if let Some(restored_root) = restored_root {
            roots.push(restored_root.to_path_buf());
        }
        Self {
            roots,
            sources: Vec::new(),
            failure: None,
        }
    }

    pub fn has_abstract_path(&self, path: &str) -> bool {
        (0..self.sources.len()).any(|index| path == format!("assets/{index}"))
    }

    pub fn package(&self) -> Result<Vec<Asset>> {
        let mut total = 0_usize;
        let mut assets = Vec::with_capacity(self.sources.len());
        for (index, source) in self.sources.iter().enumerate() {
            let file = open_regular(&source.root, &source.relative)?;
            let size = usize::try_from(file.metadata().map_err(|e| error(e.to_string()))?.len())
                .map_err(|_| error("LV2 asset size exceeds platform limit"))?;
            total = total.saturating_add(size);
            if total > MAX_ASSET_BYTES {
                return Err(error("LV2 assets exceed 40 MiB"));
            }
            let mut data = Vec::with_capacity(size);
            file.take((size + 1) as u64)
                .read_to_end(&mut data)
                .map_err(|e| error(e.to_string()))?;
            if data.len() != size {
                return Err(error("LV2 asset changed during capture"));
            }
            assets.push(Asset {
                path: format!("assets/{index}"),
                sha256: format!("{:x}", Sha256::digest(&data)),
                data: STANDARD.encode(data),
            });
        }
        Ok(assets)
    }

    fn map_absolute(&mut self, raw: *const c_char) -> Result<*mut c_char> {
        let path = c_path(raw)?;
        if !path.is_absolute() {
            return Err(error("LV2 mapPath requires an absolute path"));
        }
        let (root, relative) = self
            .roots
            .iter()
            .find_map(|root| {
                path.strip_prefix(root)
                    .ok()
                    .map(|relative| (root, relative))
            })
            .ok_or_else(|| error("LV2 asset path is outside the host-owned state directory"))?;
        validate_relative(relative)?;
        open_regular(root, relative)?;
        if let Some(index) = self.sources.iter().position(|source| {
            source.root.as_path() == root && source.relative.as_path() == relative
        }) {
            return malloc_string(&format!("assets/{index}"));
        }
        if self.sources.len() >= MAX_ASSETS {
            return Err(error("LV2 asset count exceeds limit"));
        }
        let index = self.sources.len();
        self.sources.push(Source {
            root: root.clone(),
            relative: relative.to_path_buf(),
        });
        malloc_string(&format!("assets/{index}"))
    }

    fn make_path(&mut self, raw: *const c_char) -> Result<*mut c_char> {
        let relative = c_path(raw)?;
        validate_relative(&relative)?;
        let root = &self.roots[0];
        ensure_parent(root, &relative)?;
        let absolute = root.join(relative);
        malloc_string(
            absolute
                .to_str()
                .ok_or_else(|| error("LV2 asset path is not UTF-8"))?,
        )
    }

    fn absolute_path(&self, raw: *const c_char) -> Result<*mut c_char> {
        let name = c_string(raw)?;
        let source = self
            .sources
            .iter()
            .enumerate()
            .find(|(index, _)| name == format!("assets/{index}"))
            .map(|(_, source)| source)
            .ok_or_else(|| error("LV2 save requested an unknown asset"))?;
        open_regular(&source.root, &source.relative)?;
        let path = source.root.join(&source.relative);
        malloc_string(
            path.to_str()
                .ok_or_else(|| error("LV2 asset path is not UTF-8"))?,
        )
    }
}

pub(super) struct RestorePaths {
    root: PathBuf,
    names: BTreeSet<String>,
    pub failure: Option<String>,
}

impl RestorePaths {
    pub fn new(root: &Path, assets: &[Asset]) -> Self {
        Self {
            root: root.to_path_buf(),
            names: assets.iter().map(|asset| asset.path.clone()).collect(),
            failure: None,
        }
    }

    fn absolute(&mut self, raw: *const c_char) -> Result<*mut c_char> {
        let name = c_string(raw)?;
        if !self.names.contains(&name) {
            return Err(error("LV2 restore requested an unknown asset"));
        }
        let relative = Path::new(&name);
        validate_relative(relative)?;
        open_regular(&self.root, relative)?;
        let path = self.root.join(relative);
        malloc_string(
            path.to_str()
                .ok_or_else(|| error("LV2 asset path is not UTF-8"))?,
        )
    }

    fn abstract_path(&self, raw: *const c_char) -> Result<*mut c_char> {
        let path = c_path(raw)?;
        if !path.is_absolute() {
            return Err(error("LV2 mapPath requires an absolute path"));
        }
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| error("LV2 asset path is outside the host-owned restore directory"))?;
        validate_relative(relative)?;
        let name = relative
            .to_str()
            .ok_or_else(|| error("LV2 asset path is not UTF-8"))?;
        if !self.names.contains(name) {
            return Err(error("LV2 restore requested an unknown asset"));
        }
        open_regular(&self.root, relative)?;
        malloc_string(name)
    }
}

pub(super) fn materialize(assets: &[Asset]) -> Result<tempfile::TempDir> {
    if assets.len() > MAX_ASSETS {
        return Err(error("LV2 asset count exceeds limit"));
    }
    let root = tempfile::Builder::new()
        .prefix("mixrelay-lv2-restore-")
        .tempdir()
        .map_err(|e| error(e.to_string()))?;
    if !assets.is_empty() {
        fs::create_dir(root.path().join("assets")).map_err(|e| error(e.to_string()))?;
    }
    let mut total = 0_usize;
    for (index, asset) in assets.iter().enumerate() {
        if asset.path != format!("assets/{index}") || asset.sha256.len() != 64 {
            return Err(error("LV2 asset identity is invalid"));
        }
        let decoded_size = decoded_size(&asset.data)?;
        total = add_asset_size(total, decoded_size)?;
        let data = STANDARD
            .decode(&asset.data)
            .map_err(|e| error(format!("invalid LV2 asset data: {e}")))?;
        if data.len() != decoded_size {
            return Err(error("LV2 asset decoded size is invalid"));
        }
        if format!("{:x}", Sha256::digest(&data)) != asset.sha256 {
            return Err(error("LV2 asset SHA-256 mismatch"));
        }
        let path = root.path().join(&asset.path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| error(e.to_string()))?;
        file.write_all(&data).map_err(|e| error(e.to_string()))?;
        file.sync_all().map_err(|e| error(e.to_string()))?;
    }
    Ok(root)
}

pub(super) fn decoded_size(encoded: &str) -> Result<usize> {
    if !encoded.len().is_multiple_of(4) {
        return Err(error("invalid LV2 asset data length"));
    }
    let padding = encoded
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'=')
        .count();
    if padding > 2 {
        return Err(error("invalid LV2 asset data padding"));
    }
    (encoded.len() / 4)
        .checked_mul(3)
        .and_then(|size| size.checked_sub(padding))
        .ok_or_else(|| error("LV2 asset size exceeds platform limit"))
}

pub(super) fn add_asset_size(total: usize, size: usize) -> Result<usize> {
    let total = total
        .checked_add(size)
        .ok_or_else(|| error("LV2 assets exceed 40 MiB"))?;
    if total > MAX_ASSET_BYTES {
        return Err(error("LV2 assets exceed 40 MiB"));
    }
    Ok(total)
}

pub(super) unsafe extern "C" fn save_abstract(
    handle: *mut c_void,
    path: *const c_char,
) -> *mut c_char {
    // SAFETY: the synchronous LV2 call owns this callback context until it returns.
    let context = unsafe { &mut *handle.cast::<SavePaths>() };
    match context.map_absolute(path) {
        Ok(value) => value,
        Err(e) => {
            context.failure = Some(e.to_string());
            std::ptr::null_mut()
        }
    }
}

pub(super) unsafe extern "C" fn save_make(handle: *mut c_void, path: *const c_char) -> *mut c_char {
    // SAFETY: the synchronous LV2 call owns this callback context until it returns.
    let context = unsafe { &mut *handle.cast::<SavePaths>() };
    match context.make_path(path) {
        Ok(value) => value,
        Err(e) => {
            context.failure = Some(e.to_string());
            std::ptr::null_mut()
        }
    }
}

pub(super) unsafe extern "C" fn save_absolute(
    handle: *mut c_void,
    path: *const c_char,
) -> *mut c_char {
    // SAFETY: save exposes this pointer only during its synchronous call; it points to SavePaths.
    let context = unsafe { &mut *handle.cast::<SavePaths>() };
    match context.absolute_path(path) {
        Ok(value) => value,
        Err(e) => {
            context.failure = Some(e.to_string());
            std::ptr::null_mut()
        }
    }
}

pub(super) unsafe extern "C" fn restore_absolute(
    handle: *mut c_void,
    path: *const c_char,
) -> *mut c_char {
    // SAFETY: restore exposes this pointer only during its synchronous call; it points to RestorePaths.
    let context = unsafe { &mut *handle.cast::<RestorePaths>() };
    match context.absolute(path) {
        Ok(value) => value,
        Err(e) => {
            context.failure = Some(e.to_string());
            std::ptr::null_mut()
        }
    }
}

pub(super) unsafe extern "C" fn restore_abstract(
    handle: *mut c_void,
    path: *const c_char,
) -> *mut c_char {
    // SAFETY: restore exposes this pointer only during its synchronous call; it points to RestorePaths.
    let context = unsafe { &mut *handle.cast::<RestorePaths>() };
    match context.abstract_path(path) {
        Ok(value) => value,
        Err(e) => {
            context.failure = Some(e.to_string());
            std::ptr::null_mut()
        }
    }
}

pub(super) unsafe extern "C" fn free_path(_handle: *mut c_void, path: *mut c_char) {
    // SAFETY: every non-null path returned by our mapPath/makePath callbacks is malloc-owned.
    unsafe { libc::free(path.cast()) };
}

fn c_string(raw: *const c_char) -> Result<String> {
    if raw.is_null() {
        return Err(error("LV2 asset path is null"));
    }
    // SAFETY: LV2 requires a valid NUL-terminated C string for path callbacks.
    unsafe { CStr::from_ptr(raw) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| error("LV2 asset path is not UTF-8"))
}

fn c_path(raw: *const c_char) -> Result<PathBuf> {
    Ok(PathBuf::from(c_string(raw)?))
}

fn malloc_string(value: &str) -> Result<*mut c_char> {
    let value = CString::new(value).map_err(|_| error("LV2 asset path contains NUL"))?;
    // SAFETY: malloc allocates the exact NUL-inclusive byte count; `value` is readable for that
    // count, and the returned allocation is released by the paired free_path callback.
    let raw = unsafe { libc::malloc(value.as_bytes_with_nul().len()) }.cast::<c_char>();
    if raw.is_null() {
        return Err(error("LV2 asset path allocation failed"));
    }
    // SAFETY: `raw` is non-null and allocated above for this exact length; CString provides a
    // non-overlapping source slice of the same length including its terminator.
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), raw, value.as_bytes_with_nul().len()) };
    Ok(raw)
}

fn validate_relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(error("LV2 asset path escapes the host-owned directory"));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn open_regular(root: &Path, relative: &Path) -> Result<File> {
    use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
    validate_relative(relative)?;
    let root_c = CString::new(root.as_os_str().as_bytes()).map_err(|_| error("NUL in root"))?;
    // SAFETY: the private root is opened as a directory without following its final symlink; a
    // successful close-on-exec descriptor is immediately transferred to one owned File below.
    let fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(error(std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: `fd` is a newly owned successful open descriptor and is consumed exactly once.
    let mut directory = unsafe { File::from_raw_fd(fd) };
    let components: Vec<_> = relative.components().collect();
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        let name = CString::new(name.as_bytes()).map_err(|_| error("NUL in asset path"))?;
        // SAFETY: `directory` is an owned directory descriptor; openat stays beneath it,
        // O_NOFOLLOW rejects a symlink for this component, and O_CLOEXEC prevents child leakage.
        let fd = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&directory),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(error(std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: `fd` is a newly owned successful openat descriptor and is consumed exactly once.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    let Component::Normal(name) = components[components.len() - 1] else {
        unreachable!()
    };
    let name = CString::new(name.as_bytes()).map_err(|_| error("NUL in asset path"))?;
    // SAFETY: the parent is the owned directory descriptor; O_NOFOLLOW rejects a final symlink,
    // O_NONBLOCK prevents a FIFO from blocking before fstat, and O_CLOEXEC prevents child leakage.
    let fd = unsafe {
        libc::openat(
            std::os::fd::AsRawFd::as_raw_fd(&directory),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(error(std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: `fd` is a newly owned successful openat descriptor and is consumed exactly once.
    let file = unsafe { File::from_raw_fd(fd) };
    if !file.metadata().map_err(|e| error(e.to_string()))?.is_file() {
        return Err(error("LV2 asset is not a regular file"));
    }
    Ok(file)
}

#[cfg(not(unix))]
pub(super) fn open_regular(_root: &Path, _relative: &Path) -> Result<File> {
    Err(error("LV2 file assets require a Unix host"))
}

#[cfg(unix)]
fn ensure_parent(root: &Path, relative: &Path) -> Result<()> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    };
    validate_relative(relative)?;
    let root_c = CString::new(root.as_os_str().as_bytes()).map_err(|_| error("NUL in root"))?;
    // SAFETY: the private root is opened as a directory without following its final symlink;
    // its close-on-exec descriptor is transferred to the owned File below.
    let fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(error(std::io::Error::last_os_error().to_string()));
    }
    // SAFETY: `fd` is a newly owned successful open descriptor and is consumed exactly once.
    let mut directory = unsafe { File::from_raw_fd(fd) };
    let components: Vec<_> = relative.components().collect();
    for component in &components[..components.len() - 1] {
        let Component::Normal(name) = component else {
            unreachable!()
        };
        let name = CString::new(name.as_bytes()).map_err(|_| error("NUL in asset path"))?;
        // SAFETY: `directory` is an owned private-root/descendant directory descriptor and `name`
        // is a single validated normal component; mkdirat cannot traverse `..` or a symlink.
        let status = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
        if status != 0
            && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error(std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: the parent is an owned directory descriptor; O_NOFOLLOW rejects a pre-existing
        // symlink in the private hierarchy, and O_CLOEXEC prevents child leakage.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(error(std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: `fd` is a newly owned successful openat descriptor and is consumed exactly once.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_parent(_root: &Path, _relative: &Path) -> Result<()> {
    Err(error("LV2 file assets require a Unix host"))
}
