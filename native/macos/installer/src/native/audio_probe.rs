//! Read-only `CoreAudio` lookup isolated from the privileged transaction process.

use super::NativeError;
use core_foundation::{base::TCFType, string::CFString};
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const DEADLINE_ELAPSED: &str = "CoreAudio probe deadline already elapsed";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HalPresence {
    Absent,
    Loaded,
}

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const std::ffi::c_void,
        data_size: *mut u32,
        data: *mut std::ffi::c_void,
    ) -> i32;
}

/// Checks the fixed `MixRelay` HAL identity. Call only in the disposable probe process.
/// # Errors
/// Returns an unsuccessful or malformed `CoreAudio` property lookup.
pub fn hal_is_loaded() -> Result<bool, NativeError> {
    let bundle = CFString::new("io.mixrelay.audio.driver");
    let qualifier = bundle.as_concrete_TypeRef();
    let address = AudioObjectPropertyAddress {
        selector: u32::from_be_bytes(*b"bidp"),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let mut plugin = 0_u32;
    let mut size = 4_u32;
    let qualifier_size = u32::try_from(std::mem::size_of_val(&qualifier))
        .map_err(|_| NativeError::Activation("invalid CFString qualifier size".to_owned()))?;
    // SAFETY: the qualifier points to a live, owned CFStringRef until this
    // synchronous call returns. The aligned, disjoint u32 output has exactly
    // the declared four-byte capacity; the address has the official C layout.
    let status = unsafe {
        AudioObjectGetPropertyData(
            1,
            &raw const address,
            qualifier_size,
            (&raw const qualifier).cast(),
            &raw mut size,
            (&raw mut plugin).cast(),
        )
    };
    classify_hal_lookup(status, size, plugin)
}

fn classify_hal_lookup(status: i32, size: u32, plugin: u32) -> Result<bool, NativeError> {
    if status != 0 || size != 4 {
        return Err(NativeError::Activation(format!(
            "CoreAudio bundle lookup failed (status {status}, size {size})"
        )));
    }
    Ok(plugin != 0)
}

fn hal_probe_command() -> Result<Command, NativeError> {
    let executable = std::env::current_exe().map_err(|error| {
        NativeError::Activation(format!("resolve CoreAudio probe executable: {error}"))
    })?;
    let mut command = Command::new(executable);
    command
        .arg("probe-hal-absence")
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    Ok(command)
}

/// Confirms that the audio server answers before a privileged file transaction.
/// # Errors
/// Returns a failed lookup or a probe timeout; neither is treated as HAL absence.
pub fn check_coreaudio_response() -> Result<(), NativeError> {
    tracing::info!("Checking CoreAudio response before changing Driver files");
    probe_hal_presence_until(&mut hal_probe_command()?, Instant::now() + PROBE_TIMEOUT)?;
    Ok(())
}

pub(super) fn wait_for_hal_absence() -> Result<(), NativeError> {
    tracing::info!("Checking removed HAL absence with a 30-second process deadline");
    wait_for_hal_state(HalPresence::Absent)
}

pub(super) fn wait_for_hal_presence() -> Result<(), NativeError> {
    tracing::info!("Checking restored HAL is loaded with a 30-second process deadline");
    wait_for_hal_state(HalPresence::Loaded)
}

fn wait_for_hal_state(expected: HalPresence) -> Result<(), NativeError> {
    wait_for_presence(
        expected,
        |deadline| probe_hal_presence_until(&mut hal_probe_command()?, deadline),
        Instant::now,
        std::thread::sleep,
    )
}

fn wait_for_presence(
    expected: HalPresence,
    mut probe: impl FnMut(Instant) -> Result<HalPresence, NativeError>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<(), NativeError> {
    let deadline = now() + PROBE_TIMEOUT;
    let mut previous = None;
    loop {
        match probe(deadline) {
            Ok(actual) if actual == expected => return Ok(()),
            result if now() >= deadline => {
                // A final attempt that only found the deadline elapsed says nothing about
                // CoreAudio; report the last real observation instead.
                let result = match (result, previous) {
                    (Err(NativeError::Activation(message)), Some(observed))
                        if message == DEADLINE_ELAPSED =>
                    {
                        observed
                    }
                    (result, _) => result,
                };
                return match result {
                    Err(error) => Err(error),
                    Ok(_) => Err(NativeError::Activation(
                        match expected {
                            HalPresence::Absent => "removed HAL is still loaded by CoreAudio",
                            HalPresence::Loaded => "restored HAL is not loaded by CoreAudio",
                        }
                        .to_owned(),
                    )),
                };
            }
            result => {
                previous = Some(result);
                sleep(PROBE_RETRY_INTERVAL);
            }
        }
    }
}

fn probe_hal_presence_until(
    command: &mut Command,
    deadline: Instant,
) -> Result<HalPresence, NativeError> {
    if Instant::now() >= deadline {
        return Err(NativeError::Activation(DEADLINE_ELAPSED.to_owned()));
    }
    let mut probe = ProbeProcess {
        child: command
            .spawn()
            .map_err(|error| NativeError::Activation(format!("start CoreAudio probe: {error}")))?,
    };
    loop {
        if let Some(status) = probe.child.try_wait().map_err(|error| {
            NativeError::Activation(format!("wait for CoreAudio probe: {error}"))
        })? {
            return match status.code() {
                Some(0) => Ok(HalPresence::Absent),
                Some(2) => Ok(HalPresence::Loaded),
                _ => Err(NativeError::Activation(format!(
                    "CoreAudio probe failed ({status})"
                ))),
            };
        }
        if Instant::now() >= deadline {
            // Unlike a deadline around a synchronous CoreAudio call, this owns
            // and can terminate/reap the blocked process, with no detached worker.
            probe.child.kill().map_err(|error| {
                NativeError::Activation(format!("stop timed-out CoreAudio probe: {error}"))
            })?;
            probe.child.wait().map_err(|error| {
                NativeError::Activation(format!("reap timed-out CoreAudio probe: {error}"))
            })?;
            return Err(NativeError::Activation(
                "CoreAudio lookup probe exceeded its 30-second deadline".to_owned(),
            ));
        }
        std::thread::sleep(PROBE_POLL_INTERVAL);
    }
}

struct ProbeProcess {
    child: Child,
}
impl Drop for ProbeProcess {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::hal_reload::{
        HostIdentity, HostProcesses, capture_hosts, reload_with_readiness,
    };
    use super::*;
    use std::{cell::Cell, path::PathBuf};

    #[test]
    fn recovery_without_driver_requires_loaded_plugin_and_falls_back_if_absent() {
        struct Hosts;
        impl HostProcesses for Hosts {
            fn candidates(&mut self) -> Result<Vec<u32>, NativeError> {
                Ok(vec![42])
            }
            fn identity(&mut self, _: u32, _: &[PathBuf]) -> Result<HostIdentity, NativeError> {
                Ok(HostIdentity {
                    started: "generation".to_owned(),
                    command: "host".to_owned(),
                })
            }
            fn signal(&mut self, _: u32, _: &'static str) -> Result<(), NativeError> {
                Ok(())
            }
            fn wait_for_exit(&mut self, _: u32) -> Result<bool, NativeError> {
                Ok(true)
            }
        }
        let mut hosts = Hosts;
        let paths = [PathBuf::from(crate::HAL_PATH)];
        let captured = capture_hosts(&mut hosts, &paths).unwrap();
        let full_reload = Cell::new(false);
        let readiness_calls = Cell::new(0);
        reload_with_readiness(
            Some(&captured),
            &paths,
            &mut hosts,
            || {
                full_reload.set(true);
                Ok(())
            },
            || {
                readiness_calls.set(readiness_calls.get() + 1);
                let clock = Cell::new(Instant::now());
                wait_for_presence(
                    HalPresence::Loaded,
                    |_| {
                        // A successful lookup with plugin == 0 answers CoreAudio
                        // preflight, but must not satisfy restored HAL readiness.
                        let plugin = if full_reload.get() { 100 } else { 0 };
                        classify_hal_lookup(0, 4, plugin).map(|loaded| {
                            if loaded {
                                HalPresence::Loaded
                            } else {
                                HalPresence::Absent
                            }
                        })
                    },
                    || clock.get(),
                    |_| clock.set(clock.get() + PROBE_TIMEOUT),
                )
            },
        )
        .unwrap();
        assert!(full_reload.get());
        assert_eq!(readiness_calls.get(), 2);
    }

    #[test]
    fn a_timed_out_wait_reports_the_last_real_probe_failure() {
        let clock = Cell::new(Instant::now());
        let error = wait_for_presence(
            HalPresence::Absent,
            |deadline| {
                if clock.get() >= deadline {
                    Err(NativeError::Activation(DEADLINE_ELAPSED.to_owned()))
                } else {
                    Err(NativeError::Activation(
                        "start CoreAudio probe: No such file or directory".to_owned(),
                    ))
                }
            },
            || clock.get(),
            |_| clock.set(clock.get() + PROBE_TIMEOUT),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("start CoreAudio probe"),
            "the reported cause is the probe failure, not the elapsed deadline: {error}"
        );
    }

    #[test]
    fn lookup_errors_and_unexpected_data_never_confirm_absence() {
        assert!(!classify_hal_lookup(0, 4, 0).unwrap());
        assert!(classify_hal_lookup(0, 4, 100).unwrap());
        assert!(classify_hal_lookup(-1, 4, 0).is_err());
        assert!(classify_hal_lookup(0, 0, 0).is_err());
        assert!(classify_hal_lookup(0, 8, 0).is_err());
    }

    #[test]
    fn probe_requires_explicit_absence_and_preserves_present_and_failure_results() {
        let deadline = || Instant::now() + Duration::from_secs(2);
        assert_eq!(
            probe_hal_presence_until(
                Command::new("/usr/bin/true").stdin(Stdio::null()),
                deadline()
            )
            .unwrap(),
            HalPresence::Absent
        );
        let mut present = Command::new("/bin/sh");
        present.args(["-c", "exit 2"]);
        assert_eq!(
            probe_hal_presence_until(&mut present, deadline()).unwrap(),
            HalPresence::Loaded
        );
        assert!(probe_hal_presence_until(&mut Command::new("/usr/bin/false"), deadline()).is_err());
        let mut signaled = Command::new("/bin/sh");
        signaled.args(["-c", "kill -TERM $$"]);
        assert!(probe_hal_presence_until(&mut signaled, deadline()).is_err());
    }

    #[test]
    fn expired_budget_never_starts_another_probe() {
        let fixture = mixrelay_test_support::FixtureDir::new("expired-coreaudio-probe").unwrap();
        let path = fixture.join("started");
        let mut command = Command::new("/usr/bin/touch");
        command.arg(&path);
        assert!(probe_hal_presence_until(&mut command, Instant::now()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn hung_probe_is_killed_and_reaped_instead_of_blocking_recovery() {
        let fixture = mixrelay_test_support::FixtureDir::new("hung-coreaudio-probe").unwrap();
        let pid_path = fixture.join("pid");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", r#"echo $$ > "$1"; exec /bin/sleep 60"#, "probe"])
            .arg(&pid_path);
        let began = Instant::now();
        assert!(
            probe_hal_presence_until(&mut command, began + Duration::from_millis(300)).is_err()
        );
        assert!(began.elapsed() < Duration::from_secs(2));
        let pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        // SAFETY: signal zero only queries the fixture process's existence.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "timed-out child must be gone, including no zombie"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
