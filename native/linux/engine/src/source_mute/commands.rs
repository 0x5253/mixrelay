//! The lease protocol with `WirePlumber`: write a lease, wait for its exact ACK, remove
//! only `WirePlumber`'s own links, and clear the lease. Every step is deadline-bounded.
use super::{
    lease::Lease,
    registry::{Host, METADATA_NAME, Registry},
};
use mixrelay_time::{Deadline, MonotonicClock, Sleeper};
use std::{io::Read, time::Duration};

const REGISTRY_READ_TIMEOUT: Duration = Duration::from_secs(2);
const INSTALL_TIMEOUT: Duration = Duration::from_millis(500);
const CLEAR_TIMEOUT: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The `PipeWire` command-line tools source muting drives.
pub(super) trait PipeWireTools: Send + Sync {
    /// Reads one registry snapshot with `pw-dump`.
    fn read_registry(&self, timeout: Duration) -> Result<Registry, String>;
    /// Runs `pw-metadata` or `pw-cli` to completion.
    fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<(), String>;
}

/// The installed tools, bounded by `mixrelay-linux-audio-io`.
struct CliTools;

impl PipeWireTools for CliTools {
    fn read_registry(&self, timeout: Duration) -> Result<Registry, String> {
        let output = mixrelay_linux_audio_io::bounded_pipewire_output("pw-dump", &[], timeout)
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "pw-dump failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Registry::parse(&output.stdout, Host::current())
    }

    fn run(&self, program: &str, args: &[String], timeout: Duration) -> Result<(), String> {
        let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = mixrelay_linux_audio_io::bounded_pipewire_output(program, &borrowed, timeout)
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{program} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// Monotonic time for deadlines and the waits between registry polls.
pub(super) trait Timer: MonotonicClock + Sleeper {}
impl<T: MonotonicClock + Sleeper> Timer for T {}

/// The tools and the time source the lease protocol runs against.
pub(super) struct PipeWireSession {
    tools: Box<dyn PipeWireTools>,
    timer: Box<dyn Timer>,
}

impl PipeWireSession {
    pub(super) fn new(tools: impl PipeWireTools + 'static, timer: impl Timer + 'static) -> Self {
        Self {
            tools: Box::new(tools),
            timer: Box::new(timer),
        }
    }

    /// The installed tools, timed by `timer`.
    pub(super) fn installed(timer: impl Timer + 'static) -> Self {
        Self::new(CliTools, timer)
    }

    pub(super) fn read_registry(&self) -> Result<Registry, String> {
        self.tools.read_registry(REGISTRY_READ_TIMEOUT)
    }

    fn deadline(&self, timeout: Duration) -> Deadline {
        Deadline::after(self.timer.now(), timeout)
    }

    /// Time left before `deadline`; a spent deadline fails before any command starts.
    fn remaining(&self, deadline: Deadline) -> Result<Duration, String> {
        Some(deadline.remaining(self.timer.now()))
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| "PipeWire source-mute command deadline expired".to_owned())
    }

    fn read_registry_until(&self, deadline: Deadline) -> Result<Registry, String> {
        self.tools.read_registry(self.remaining(deadline)?)
    }

    fn run_until(&self, program: &str, args: &[String], deadline: Deadline) -> Result<(), String> {
        self.tools.run(program, args, self.remaining(deadline)?)
    }

    /// Waits one poll interval, never past `deadline`.
    fn pause(&self, deadline: Deadline) -> Result<(), String> {
        let wait = self.remaining(deadline)?.min(POLL_INTERVAL);
        self.timer.sleep_until(self.deadline(wait));
        Ok(())
    }

    /// Installs only after `WirePlumber`'s matching ACK, then removes only existing
    /// `WirePlumber`-owned links from the selected stream to this owned endpoint.
    /// A failed install clears its lease again.
    pub(super) fn install(&self, lease: &mut Lease) -> Result<(), String> {
        let deadline = self.deadline(INSTALL_TIMEOUT);
        pin_route(lease, &self.read_registry_until(deadline)?)?;
        let result = self.acknowledge_and_unlink(lease, deadline);
        if result.is_err()
            && let Err(error) = self.metadata_clear(lease)
        {
            tracing::warn!(%error, "could not compensate failed source-mute installation");
        }
        result
    }

    fn acknowledge_and_unlink(&self, lease: &Lease, deadline: Deadline) -> Result<(), String> {
        self.run_until("pw-metadata", &metadata_set_args(lease), deadline)?;
        loop {
            let registry = self.read_registry_until(deadline)?;
            if registry.ack(lease) {
                if !lease.owners_unchanged(&registry) {
                    break;
                }
                let links = registry.links(lease);
                if links.iter().any(|(_, managed)| !managed) {
                    break;
                }
                for (id, _) in links {
                    self.run_until("pw-cli", &["destroy".into(), id.to_string()], deadline)?;
                }
                if lease.still_valid(&self.read_registry_until(deadline)?) {
                    return Ok(());
                }
                break;
            }
            self.pause(deadline)?;
        }
        Err("WirePlumber did not acknowledge an isolated owned playback route".into())
    }

    /// Deletes the lease key and waits until both the key and its ACK are gone.
    pub(super) fn metadata_clear(&self, lease: &Lease) -> Result<(), String> {
        let deadline = self.deadline(CLEAR_TIMEOUT);
        self.run_until("pw-metadata", &metadata_clear_args(lease), deadline)?;
        let key = lease_key(lease);
        loop {
            let registry = self.read_registry_until(deadline)?;
            if registry.metadata_value(lease.endpoint_id, &key).is_none() && !registry.ack(lease) {
                return Ok(());
            }
            self.pause(deadline)?;
        }
    }
}

/// Records the routes the source has at install. Later routes revoke the lease.
fn pin_route(lease: &mut Lease, registry: &Registry) -> Result<(), String> {
    lease.other_sink_serials = registry
        .other_sink_serials(lease)
        .ok_or("could not pin source playback sink routes")?;
    lease.routing_target = registry
        .routing_target(lease.source_id)
        .ok_or("could not pin WirePlumber playback routing target")?
        .into_option();
    lease.source_target_hint = registry
        .source_target_hint(lease.source_id)
        .ok_or("could not pin source playback routing hint")?
        .into_option();
    Ok(())
}

fn lease_key(lease: &Lease) -> String {
    format!("lease.{}", lease.key.source_serial)
}

fn metadata_set_args(lease: &Lease) -> [String; 5] {
    [
        "-n".into(),
        METADATA_NAME.into(),
        lease.endpoint_id.to_string(),
        lease_key(lease),
        lease.value(),
    ]
}

fn metadata_clear_args(lease: &Lease) -> [String; 5] {
    [
        "-d".into(),
        "-n".into(),
        METADATA_NAME.into(),
        lease.endpoint_id.to_string(),
        lease_key(lease),
    ]
}

/// 128 random bits from the kernel, hex-encoded.
pub(super) fn new_nonce() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|e| e.to_string())?;
    let mut nonce = String::with_capacity(32);
    for byte in bytes {
        nonce.push(char::from(HEX[usize::from(byte >> 4)]));
        nonce.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(nonce)
}

#[cfg(test)]
mod tests;
