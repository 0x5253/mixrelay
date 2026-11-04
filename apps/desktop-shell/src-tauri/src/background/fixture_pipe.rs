//! Named pipes that let controller fixture scripts wait on a test without timers.
use std::{
    ffi::CString,
    fs::File,
    io::{Read as _, Write as _},
    os::{fd::AsRawFd as _, unix::ffi::OsStrExt as _},
    path::Path,
};

/// Bounds each wait for the fixture; a healthy fixture answers in milliseconds.
const FIXTURE_TIMEOUT_MS: libc::c_int = 5_000;

/// A named pipe the test holds open for reading and writing. Neither side's open blocks,
/// and the fixture reads end-of-file once the test drops it, even after a failed assertion.
pub(super) struct Pipe(File);

impl Pipe {
    pub(super) fn create(path: &Path) -> Self {
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is a live NUL-terminated path for the duration of the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        Self(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap(),
        )
    }

    /// Waits for the fixture's next message.
    pub(super) fn receive(&mut self, expected: &str) {
        assert!(
            self.readable(FIXTURE_TIMEOUT_MS),
            "the controller fixture did not answer"
        );
        let mut message = vec![0; expected.len()];
        self.0.read_exact(&mut message).unwrap();
        assert_eq!(message, expected.as_bytes());
    }

    pub(super) fn send(&mut self, message: &str) {
        self.0.write_all(message.as_bytes()).unwrap();
    }

    /// Whether the fixture has written within `timeout_ms`; zero only inspects.
    pub(super) fn readable(&self, timeout_ms: libc::c_int) -> bool {
        let mut pending = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pending` is one valid pollfd for a descriptor this pipe owns.
        let events = unsafe { libc::poll(&raw mut pending, 1, timeout_ms) };
        assert!(events >= 0, "{}", std::io::Error::last_os_error());
        events == 1
    }
}
