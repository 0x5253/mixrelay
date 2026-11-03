//! Linux process identity is PID plus kernel start time, never a display name.
use std::{fs, io::Read, path::Path};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub process_id: u32,
    pub parent_id: u32,
    pub start_time: u64,
    pub executable_path: Option<String>,
}

/// Read-only, bounded `/proc` lookup. Recheck the birth stamp around readlink so
/// process exit/PID reuse cannot combine two processes' identity fields.
#[must_use]
pub fn process_identity(process_id: u32) -> Option<ProcessIdentity> {
    inspect(&ProcFiles, process_id)
}

trait ProcessFiles {
    fn stat(&self, process_id: u32) -> Option<String>;
    fn executable(&self, process_id: u32) -> Option<String>;
}

struct ProcFiles;
impl ProcessFiles for ProcFiles {
    fn stat(&self, process_id: u32) -> Option<String> {
        let mut bytes = Vec::new();
        fs::File::open(format!("/proc/{process_id}/stat"))
            .ok()?
            .take(4097)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= 4096)
            .then(|| String::from_utf8(bytes).ok())
            .flatten()
    }
    fn executable(&self, process_id: u32) -> Option<String> {
        fs::read_link(format!("/proc/{process_id}/exe"))
            .ok()?
            .to_str()
            .map(str::to_owned)
    }
}

fn inspect(files: &impl ProcessFiles, process_id: u32) -> Option<ProcessIdentity> {
    let stamp = || parse_stat(&files.stat(process_id)?);
    let (parent_id, start_time) = stamp()?;
    let executable_path = files.executable(process_id).filter(|value| {
        Path::new(value).is_absolute() && value.len() <= 4096 && !value.ends_with(" (deleted)")
    });
    if stamp()? != (parent_id, start_time) {
        return None;
    }
    Some(ProcessIdentity {
        process_id,
        parent_id,
        start_time,
        executable_path,
    })
}

fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let fields = stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

#[must_use]
pub fn same_process(left: &ProcessIdentity, right: &ProcessIdentity) -> bool {
    left.process_id == right.process_id && left.start_time == right.start_time
}

/// A namespace-local or forged `PipeWire` PID must not turn an unrelated host
/// executable into persistent application intent.
#[must_use]
pub fn verified_executable(process_id: u32, reported_binary: Option<&str>) -> Option<String> {
    verified_identity_executable(process_identity(process_id)?, reported_binary)
}

fn verified_identity_executable(
    identity: ProcessIdentity,
    reported_binary: Option<&str>,
) -> Option<String> {
    let path = identity.executable_path?;
    let reported = reported_binary?;
    let matches = if Path::new(reported).is_absolute() {
        path == reported
    } else {
        Path::new(&path).file_name().and_then(|name| name.to_str()) == Some(reported)
    };
    matches.then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stat_parser_handles_spaces_and_parentheses_and_rejects_truncation() {
        let line = format!(
            "123 (a ) strange name) S 42 {} 987",
            vec!["0"; 17].join(" ")
        );
        assert_eq!(parse_stat(&line), Some((42, 987)));
        assert!(parse_stat("123 (x) S 42").is_none());
    }
    fn stat(parent: u32, start: u64) -> String {
        format!(
            "123 (a ) strange name) S {parent} {} {start}",
            vec!["0"; 17].join(" ")
        )
    }
    struct Files {
        stamps: std::cell::RefCell<std::collections::VecDeque<Option<String>>>,
        path: Option<String>,
    }
    impl ProcessFiles for Files {
        fn stat(&self, _: u32) -> Option<String> {
            self.stamps.borrow_mut().pop_front().flatten()
        }
        fn executable(&self, _: u32) -> Option<String> {
            self.path.clone()
        }
    }
    #[test]
    fn pid_reuse_or_exit_never_combines_another_process_executable() {
        for after in [Some(stat(42, 200)), None] {
            let files = Files {
                stamps: std::cell::RefCell::new([Some(stat(42, 100)), after].into()),
                path: Some("/usr/bin/player".into()),
            };
            assert_eq!(inspect(&files, 123), None);
        }
        let files = Files {
            stamps: std::cell::RefCell::new([Some(stat(42, 100)), Some(stat(42, 100))].into()),
            path: Some("/usr/bin/player".into()),
        };
        let identity = inspect(&files, 123).unwrap();
        assert_eq!(
            verified_identity_executable(identity.clone(), Some("player")),
            Some("/usr/bin/player".into())
        );
        assert_eq!(verified_identity_executable(identity, Some("other")), None);
    }
    #[test]
    fn unavailable_deleted_relative_and_oversized_paths_do_not_create_persistent_intent() {
        for path in [
            None,
            Some("/usr/bin/player (deleted)".into()),
            Some("relative/player".into()),
            Some(format!("/{}", "x".repeat(4096))),
        ] {
            let files = Files {
                stamps: std::cell::RefCell::new([Some(stat(42, 100)), Some(stat(42, 100))].into()),
                path,
            };
            let identity = inspect(&files, 123).unwrap();
            assert_eq!(identity.executable_path, None);
        }
    }
    #[cfg(all(target_os = "linux", feature = "native-lab"))]
    #[test]
    fn current_process_read_is_stable_and_wrong_binary_is_not_verified() {
        let pid = std::process::id();
        let identity = process_identity(pid).expect("own proc identity");
        assert!(same_process(&identity, &process_identity(pid).unwrap()));
        assert!(verified_executable(pid, Some("definitely-not-mixrelay")).is_none());
        let executable = identity.executable_path.unwrap();
        assert_eq!(
            verified_executable(pid, Some(&executable)),
            Some(executable)
        );
    }
}
