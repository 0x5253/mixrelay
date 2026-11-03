//! Desktop diagnostics go to a log file per process: the app has no terminal, so an
//! unrecorded failure (such as a runtime that never starts) could not be diagnosed.
//! Each process owns its file, so concurrent launches never rotate one another's log;
//! a file is capped while it is written and only the newest few per role are kept.
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use mixrelay_desktop_application::log_retention::{LogEntry, log_pid, obsolete_logs};
use tracing_subscriber::{EnvFilter, filter::LevelFilter};

/// A running process's log restarts from empty past this size.
const CAP_BYTES: u64 = 5 * 1024 * 1024;
/// Logs of exited processes kept per role, including the one being opened; logs of live
/// processes are always kept.
const KEEP: usize = 5;

pub(crate) fn init(role: &str) {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    let file = std::env::var_os("HOME")
        .and_then(|home| open(&directory(Path::new(&home)), role, std::process::id()));
    // A second initialization (tests, re-entry) keeps the first subscriber.
    let _ = match file {
        Some(file) => builder.with_writer(Mutex::new(file)).try_init(),
        None => builder.with_writer(io::stderr).try_init(),
    };
}

fn directory(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/MixRelay")
    } else {
        home.join(".local/state/mixrelay/logs")
    }
}

fn open(directory: &Path, role: &str, pid: u32) -> Option<CappedFile> {
    std::fs::create_dir_all(directory).ok()?;
    prune(directory, role);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(directory.join(format!("{role}-{pid}.log")))
        .ok()?;
    Some(CappedFile {
        written: file.metadata().map_or(0, |metadata| metadata.len()),
        file,
        cap: CAP_BYTES,
    })
}

/// Removes the oldest logs of `role`, leaving room for the one about to be opened.
fn prune(directory: &Path, role: &str) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let logs = entries.flatten().filter_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        let pid = log_pid(&name, role)?;
        Some(LogEntry {
            name,
            modified: entry.metadata().ok()?.modified().ok()?,
            live: alive(pid),
            token: entry.path(),
        })
    });
    for path in obsolete_logs(role, KEEP, logs) {
        let _ = std::fs::remove_file(path);
    }
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 performs only the existence and permission check.
    let found = unsafe { libc::kill(pid, 0) } == 0;
    found || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

struct CappedFile {
    file: File,
    written: u64,
    cap: u64,
}

impl Write for CappedFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.written + buffer.len() as u64 > self.cap {
            // Appends follow the new end, so restarting the file keeps it bounded.
            self.file.set_len(0)?;
            self.written = 0;
        }
        let count = self.file.write(buffer)?;
        self.written += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Release webviews have no developer tools: uncaught page errors are kept in this log.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri deserializes command arguments into owned values"
)]
pub(crate) fn webview_error(message: String) {
    let message: String = message.chars().take(4096).collect();
    // Debug formatting escapes line breaks, so page text cannot forge log entries.
    tracing::error!(?message, "webview error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn webview_errors_stay_one_bounded_line() {
        #[derive(Clone, Default)]
        struct Buffer(std::sync::Arc<Mutex<Vec<u8>>>);
        impl io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            webview_error(format!(
                "boom\n2026-10-15T00:00:00Z INFO forged{}",
                "x".repeat(5000)
            ));
        });
        let output = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
        assert_eq!(output.lines().count(), 1, "{output}");
        assert!(output.contains("boom\\n2026"));
        assert!(output.len() < 4400);
    }

    #[test]
    fn writes_a_private_log_per_process_and_keeps_the_newest_per_role() {
        let home = tempfile::tempdir().unwrap();
        let directory = directory(home.path());
        // Beyond any real PID, so these stand for exited processes.
        let gone = 3_000_000_000_u32;
        for pid in gone + 1..=gone + 7 {
            let mut file = open(&directory, "desktop", pid).unwrap();
            writeln!(file, "process {pid}").unwrap();
            file.file
                .set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(u64::from(pid - gone)),
                )
                .unwrap();
        }
        std::fs::write(directory.join("desktop-notes.log"), b"other").unwrap();
        // The current test process is alive: its old log survives pruning.
        let live = directory.join(format!("desktop-{}.log", std::process::id()));
        std::fs::write(&live, b"live").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH;
        File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(old)
            .unwrap();
        open(&directory, "desktop", gone + 8).unwrap();
        std::fs::write(directory.join("driver-runtime-1.log"), b"other role").unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        let mut expected = vec![
            format!("desktop-{}.log", gone + 4),
            format!("desktop-{}.log", gone + 5),
            format!("desktop-{}.log", gone + 6),
            format!("desktop-{}.log", gone + 7),
            format!("desktop-{}.log", gone + 8),
            format!("desktop-{}.log", std::process::id()),
            "desktop-notes.log".to_owned(),
            "driver-runtime-1.log".to_owned(),
        ];
        expected.sort();
        assert_eq!(names, expected);
        let path = directory.join(format!("desktop-{}.log", gone + 7));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("process {}\n", gone + 7)
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn restarts_a_running_log_that_would_pass_the_cap() {
        let home = tempfile::tempdir().unwrap();
        let mut file = open(home.path(), "desktop", 1).unwrap();
        file.cap = 8;
        file.write_all(b"12345").unwrap();
        file.write_all(b"6789").unwrap();
        file.flush().unwrap();
        let path = home.path().join("desktop-1.log");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "6789");
    }

    #[test]
    fn has_no_file_where_the_directory_cannot_be_created() {
        let home = tempfile::tempdir().unwrap();
        let blocked = home.path().join("file");
        std::fs::write(&blocked, b"").unwrap();
        assert!(open(&blocked.join("logs"), "desktop", 1).is_none());
        // An unreadable directory leaves nothing to prune.
        prune(&blocked.join("missing"), "desktop");
    }
}
