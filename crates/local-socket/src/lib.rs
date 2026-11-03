//! User-owned Unix sockets for `MixRelay`'s local control services.
//!
//! Every service socket lives in an owner-only directory (`0700`) with mode `0600`, and
//! accepted connections are admitted only from allowed user IDs, so other local users and
//! remote hosts cannot reach the engine or Driver controls. A live socket is never
//! replaced; a stale one is removed only when it is a socket owned by this user; and a
//! service removes only the socket it created.
#![cfg(unix)]

use std::io;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _,
    PermissionsExt as _,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};

/// The engine control service socket name.
pub const ENGINE_SOCKET: &str = "engine.sock";
/// The Driver service socket name.
pub const DRIVER_SOCKET: &str = "driver.sock";
/// Overrides the socket directory (absolute), so tests and labs never touch a session's
/// installed services.
pub const DIRECTORY_VARIABLE: &str = "MIXRELAY_IPC_DIR";
/// The logical HTTP authority clients send over the sockets; it names no network address.
pub const ENGINE_AUTHORITY: &str = "http://mixrelay-engine";
/// The logical HTTP authority of the Driver service.
pub const DRIVER_AUTHORITY: &str = "http://mixrelay-driver";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// This process's effective user ID.
#[must_use]
pub fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The socket directory a service binds: the override when set (an isolated test or lab
/// launch), otherwise the session's.
///
/// # Errors
/// Fails when neither the override nor the platform's per-user location is available.
pub fn socket_directory() -> io::Result<PathBuf> {
    directory_from(
        std::env::var_os(DIRECTORY_VARIABLE).map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    )
}

/// The session's socket directory, ignoring the override. The desktop app uses this: the
/// installed services (launchd, systemd) never see an override set in the app's
/// environment, so honouring it there would split the app from its services.
///
/// # Errors
/// Fails when the platform's per-user location is not available.
pub fn session_socket_directory() -> io::Result<PathBuf> {
    directory_from(
        None,
        std::env::var_os("HOME").map(PathBuf::from),
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
    )
}

/// The socket directory of a user with the given home, for a privileged process acting
/// for that user (macOS installer). Linux sessions use their runtime directory instead.
#[must_use]
pub fn socket_directory_in_home(home: &Path) -> PathBuf {
    home.join("Library/Application Support/MixRelay/ipc")
}

fn directory_from(
    explicit: Option<PathBuf>,
    home: Option<PathBuf>,
    runtime: Option<PathBuf>,
) -> io::Result<PathBuf> {
    let absolute = |path: PathBuf, name: &str| {
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be an absolute path"),
            ))
        }
    };
    if let Some(explicit) = explicit {
        return absolute(explicit, DIRECTORY_VARIABLE);
    }
    platform_directory(home, runtime, absolute)
}

#[cfg(target_os = "macos")]
fn platform_directory(
    home: Option<PathBuf>,
    _runtime: Option<PathBuf>,
    absolute: impl Fn(PathBuf, &str) -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let home = home.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    absolute(home, "HOME").map(|home| socket_directory_in_home(&home))
}

#[cfg(not(target_os = "macos"))]
fn platform_directory(
    _home: Option<PathBuf>,
    runtime: Option<PathBuf>,
    absolute: impl Fn(PathBuf, &str) -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let runtime = runtime
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    absolute(runtime, "XDG_RUNTIME_DIR").map(|runtime| runtime.join("mixrelay"))
}

/// A bound socket path; dropping it removes the socket if it is still the one bound here.
#[derive(Debug)]
pub struct OwnedSocket {
    path: PathBuf,
    inode: u64,
    // Linux may immediately reuse an unlinked inode. Retain its VFS identity until this
    // guard is dropped so it cannot mistake a replacement socket for its own.
    #[cfg(target_os = "linux")]
    _inode_handle: std::fs::File,
}

impl OwnedSocket {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.ino() == self.inode && metadata.file_type().is_socket())
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Binds `name` inside `directory`, which is created owner-only if missing and must be a
/// real directory owned by `uid` with mode `0700`. Every ancestor of its canonical path
/// must be owned by `uid` or root and writable by nobody else (root-owned sticky
/// directories such as `/tmp` excepted), so no other user can move the directory or
/// substitute a socket. Starting services serialize on a lock beside the socket: a live
/// socket is never replaced, and a stale socket owned by `uid` is replaced by one starter.
///
/// # Errors
/// Rejects unsafe directories or ancestors, foreign or non-socket paths and live sockets,
/// and reports bind failures.
pub async fn bind(
    directory: &Path,
    name: &str,
    uid: u32,
) -> io::Result<(UnixListener, OwnedSocket)> {
    let directory = secure_directory(directory, uid)?;
    let _lock = lock(&directory.join(format!("{name}.lock"))).await?;
    let path = directory.join(name);
    let Ok(existing) = std::fs::symlink_metadata(&path) else {
        return publish(path);
    };
    if !existing.file_type().is_socket() || existing.uid() != uid {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "local control path is not an owned socket",
        ));
    }
    match tokio::time::timeout(CONNECT_TIMEOUT, UnixStream::connect(&path)).await {
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
            std::fs::remove_file(&path).and_then(|()| publish(path))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "local control socket is active",
        )),
    }
}

/// Takes the starters' lock without blocking the executor, which may also be running the
/// starter that holds it.
async fn lock(path: &Path) -> io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)?;
    loop {
        match file.try_lock().map_err(io::Error::from) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            result => return result.map(|()| file),
        }
    }
}

/// Creates the directory if needed and returns its canonical path once it and all its
/// ancestors are safe for `uid` (see [`bind`]).
fn secure_directory(directory: &Path, uid: u32) -> io::Result<PathBuf> {
    let metadata = std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(directory)
        .and_then(|()| std::fs::symlink_metadata(directory))?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o777 != 0o700 {
        return Err(unsafe_path(
            "local control directory must be owned by this user with mode 0700",
        ));
    }
    std::fs::canonicalize(directory).and_then(|canonical| {
        canonical
            .ancestors()
            .skip(1)
            .try_for_each(|ancestor| {
                std::fs::metadata(ancestor).and_then(|metadata| trusted(&metadata, uid))
            })
            .map(|()| canonical)
    })
}

/// An ancestor nobody but `uid` or root can change entries in.
fn trusted(metadata: &std::fs::Metadata, uid: u32) -> io::Result<()> {
    let owner_trusted = metadata.uid() == uid || metadata.uid() == 0;
    let others_write = metadata.mode() & 0o022 != 0;
    let root_sticky = metadata.uid() == 0 && metadata.mode() & 0o1000 != 0;
    if owner_trusted && (!others_write || root_sticky) {
        Ok(())
    } else {
        Err(unsafe_path(
            "an ancestor of the local control directory is writable by another user",
        ))
    }
}

fn unsafe_path(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason.to_owned())
}

/// Binds the vacant path and takes ownership of the socket it creates.
fn publish(path: PathBuf) -> io::Result<(UnixListener, OwnedSocket)> {
    let listener = UnixListener::bind(&path)?;
    std::fs::symlink_metadata(&path)
        .and_then(|metadata| owned(path, metadata.ino()))
        .and_then(|owner| {
            std::fs::set_permissions(&owner.path, std::fs::Permissions::from_mode(0o600))
                .map(|()| (listener, owner))
        })
}

#[cfg(target_os = "linux")]
fn owned(path: PathBuf, inode: u64) -> io::Result<OwnedSocket> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map(|handle| OwnedSocket {
            path,
            inode,
            _inode_handle: handle,
        })
}

#[cfg(not(target_os = "linux"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Linux also opens an inode handle, which can fail"
)]
fn owned(path: PathBuf, inode: u64) -> io::Result<OwnedSocket> {
    Ok(OwnedSocket { path, inode })
}

/// True when the connected peer runs as one of `allowed`. Credential errors refuse.
#[must_use]
pub fn peer_allowed(stream: &UnixStream, allowed: &[u32]) -> bool {
    stream
        .peer_cred()
        .is_ok_and(|peer| allowed.contains(&peer.uid()))
}

/// The process on the other end of an accepted connection, recorded at accept time.
/// Servers read it per request through axum's `ConnectInfo<Peer>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    /// The macOS audit token, which names the exact process incarnation (for example
    /// to check its code signature). `None` elsewhere or when the kernel refused it.
    pub audit_token: Option<[u8; 32]>,
}

impl Peer {
    fn of(stream: &UnixStream, uid: u32) -> Self {
        Self {
            uid,
            audit_token: audit_token(stream),
        }
    }
}

#[cfg(target_os = "macos")]
fn audit_token(stream: &UnixStream) -> Option<[u8; 32]> {
    use std::os::fd::AsRawFd as _;
    // <sys/un.h>: SOL_LOCAL and LOCAL_PEERTOKEN.
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERTOKEN: libc::c_int = 6;
    let mut token = [0_u8; 32];
    let mut length: libc::socklen_t = 32;
    // SAFETY: the descriptor is live for this borrow; the kernel writes at most `length`
    // bytes into the local buffer and reports the written length.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    (result == 0 && length == 32).then_some(token)
}

#[cfg(not(target_os = "macos"))]
fn audit_token(_: &UnixStream) -> Option<[u8; 32]> {
    None
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, AuthorizedListener>>
    for Peer
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, AuthorizedListener>) -> Self {
        *stream.remote_addr()
    }
}

/// A Unix listener that hands axum only connections from allowed user IDs; others are
/// closed before any request is read. Each connection's address is its [`Peer`].
#[derive(Debug)]
pub struct AuthorizedListener {
    listener: UnixListener,
    allowed: Vec<u32>,
    /// Removes the socket when the listener goes away.
    socket: OwnedSocket,
}

impl AuthorizedListener {
    #[must_use]
    pub fn new(listener: UnixListener, socket: OwnedSocket, allowed: Vec<u32>) -> Self {
        Self {
            listener,
            allowed,
            socket,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.socket.path()
    }
}

impl axum::serve::Listener for AuthorizedListener {
    type Io = UnixStream;
    type Addr = Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, _) = axum::serve::Listener::accept(&mut self.listener).await;
            // Credential errors refuse, as in [`peer_allowed`].
            match stream.peer_cred() {
                Ok(credentials) if self.allowed.contains(&credentials.uid()) => {
                    let peer = Peer::of(&stream, credentials.uid());
                    return (stream, peer);
                }
                _ => tracing::warn!("refused a local control connection from another user"),
            }
        }
    }

    /// This process, which owns the listening socket.
    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(Peer {
            uid: current_uid(),
            audit_token: None,
        })
    }
}

#[cfg(test)]
mod tests;
