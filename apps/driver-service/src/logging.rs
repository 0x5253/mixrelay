//! Service diagnostics: macOS keeps fatal launchd diagnostics in the system log.

/// Installs the process-wide tracing subscriber for the service.
pub fn init() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: syslog retains the static identifier, and the process owns this logger.
        unsafe {
            libc::openlog(
                c"io.mixrelay.audio.driver".as_ptr(),
                libc::LOG_PID,
                libc::LOG_DAEMON,
            );
        };
    }
    let subscriber = tracing_subscriber::fmt().with_ansi(false);
    #[cfg(target_os = "macos")]
    let subscriber = subscriber.with_writer(ServiceLog);
    #[cfg(target_os = "linux")]
    let subscriber = subscriber.with_writer(std::io::stderr);
    subscriber.init();
}

#[cfg(target_os = "macos")]
struct ServiceLog;

#[cfg(target_os = "macos")]
struct ServiceLogWriter(i32);

#[cfg(target_os = "macos")]
impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for ServiceLog {
    type Writer = ServiceLogWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        ServiceLogWriter(libc::LOG_INFO)
    }

    fn make_writer_for(&'writer self, metadata: &tracing::Metadata<'_>) -> Self::Writer {
        let priority = match *metadata.level() {
            tracing::Level::ERROR => libc::LOG_ERR,
            tracing::Level::WARN => libc::LOG_WARNING,
            tracing::Level::INFO => libc::LOG_INFO,
            tracing::Level::DEBUG | tracing::Level::TRACE => libc::LOG_DEBUG,
        };
        ServiceLogWriter(priority)
    }
}

#[cfg(target_os = "macos")]
impl std::io::Write for ServiceLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let message = std::ffi::CString::new(String::from_utf8_lossy(buffer).replace('\0', "\\0"))?;
        // SAFETY: the fixed format accepts exactly one live terminated string pointer.
        // syslog copies its text during the call; log text never becomes a format string.
        unsafe { libc::syslog(self.0, c"%s".as_ptr(), message.as_ptr()) };
        std::io::stderr().write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stderr().flush()
    }
}
