use std::{collections::BTreeMap, io, process::Output, time::Duration};
#[path = "application_metadata.rs"]
mod application_metadata;
#[path = "command.rs"]
mod command;
use crate::process_identity;

use serde_json::Value;

use mixrelay_audio_io::{
    ApplicationSelector, AudioDeviceDescriptor, CapturableApplication, DeviceDirection,
    DeviceSelector, HostAudioBackendError,
};

/// Runs `pw-dump`, `pw-metadata`, or `pw-cli` from an off-RT control thread.
///
/// Passes `args` directly as process arguments without a shell. The nonzero `timeout` starts
/// before child creation and pipe setup; a kernel-blocked spawn itself cannot be interrupted.
/// Returns `Output` with the child's exit status, stdout (at most 16 MiB), and stderr (at most
/// 256 KiB). The owned child is killed and reaped on ordinary timeout or output-limit failure.
///
/// # Errors
///
/// Rejects other commands or a spent deadline, and forwards timeout, output-limit, and child errors.
pub fn bounded_pipewire_output(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> io::Result<Output> {
    if !matches!(program, "pw-dump" | "pw-metadata" | "pw-cli") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported PipeWire CLI",
        ));
    }
    command::bounded_output(program, args, timeout)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeWireTarget {
    pub object_id: u32,
    pub serial: String,
    /// Persistent selection uses node.name; serial is resolved only for this session.
    pub node_name: Option<String>,
    pub process_id: Option<u32>,
    pub application_id: Option<String>,
    pub executable: Option<String>,
    pub icon_name: Option<String>,
    pub display_name: String,
    pub media_class: String,
    pub channels: u32,
}

/// Reads one consistent `PipeWire` registry snapshot through the installed native CLI.
///
/// Keeping JSON parsing here gives the desktop discovery and engine capture backend the same
/// stable selector semantics without linking the portable graph crates to `libpipewire`.
///
/// # Errors
///
/// Returns a backend error when `pw-dump` is missing, fails, or emits invalid JSON.
pub fn targets() -> Result<Vec<PipeWireTarget>, HostAudioBackendError> {
    let output = command::output("pw-dump", &[])
        .map_err(|error| backend_failure(format!("could not start pw-dump: {error}")))?;
    if !output.status.success() {
        return Err(backend_failure(format!(
            "pw-dump failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let values = parse_registry_dump(&output.stdout)
        .map_err(|error| backend_failure(format!("pw-dump returned invalid JSON: {error}")))?;
    Ok(parse_targets(&values))
}

// A registry change during enumeration can append another JSON array even
// without --monitor. Merge by object identity rather than rejecting that valid
// event stream or silently ignoring updates/removals from the snapshot.
fn parse_registry_dump(bytes: &[u8]) -> Result<Vec<Value>, serde_json::Error> {
    let mut objects = BTreeMap::new();
    let mut saw_batch = false;
    for batch in serde_json::Deserializer::from_slice(bytes).into_iter::<Vec<Value>>() {
        saw_batch = true;
        for object in batch? {
            let id = object.get("id").and_then(Value::as_u64).ok_or_else(|| {
                <serde_json::Error as serde::de::Error>::custom("missing PipeWire object identity")
            })?;
            if object.get("info") == Some(&Value::Null) {
                objects.remove(&id);
            } else {
                merge_registry_value(objects.entry(id).or_insert(Value::Null), object);
            }
        }
    }
    if !saw_batch {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "empty PipeWire response",
        ));
    }
    Ok(objects.into_values().collect())
}

fn merge_registry_value(current: &mut Value, update: Value) {
    match (current, update) {
        (Value::Object(old), Value::Object(new)) => {
            for (key, value) in new {
                merge_registry_value(old.entry(key).or_insert(Value::Null), value);
            }
        }
        (current, update) => *current = update,
    }
}

/// Returns addressable `PipeWire` application output streams.
///
/// # Errors
///
/// Returns a backend error when the registry cannot be inspected.
pub fn applications(
    clock: &impl mixrelay_time::MonotonicClock,
) -> Result<Vec<CapturableApplication>, HostAudioBackendError> {
    ApplicationDiscovery::new(ClockRef(clock)).applications()
}

struct ClockRef<'a, C>(&'a C);
impl<C: mixrelay_time::MonotonicClock> mixrelay_time::MonotonicClock for ClockRef<'_, C> {
    fn now(&self) -> mixrelay_time::Instant {
        self.0.now()
    }
}

/// Application inventory with a bounded metadata cache owned by its caller.
pub struct ApplicationDiscovery<C> {
    resolver: crate::metadata_cache::Resolver<
        C,
        application_metadata::DesktopEnvironment,
        application_metadata::DesktopFiles,
    >,
}

impl<C> std::fmt::Debug for ApplicationDiscovery<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApplicationDiscovery")
            .finish_non_exhaustive()
    }
}

impl<C: mixrelay_time::MonotonicClock> ApplicationDiscovery<C> {
    /// Selects the clock used to expire positive and negative metadata entries.
    #[must_use]
    pub const fn new(clock: C) -> Self {
        Self {
            resolver: crate::metadata_cache::Resolver::new(
                clock,
                application_metadata::DesktopEnvironment,
                application_metadata::DesktopFiles,
            ),
        }
    }
    /// Reads the current application streams and resolves passive desktop metadata.
    /// # Errors
    /// Returns a native registry inspection failure.
    pub fn applications(&self) -> Result<Vec<CapturableApplication>, HostAudioBackendError> {
        Ok(application_inventory(targets()?, &self.resolver))
    }
}

fn application_inventory<
    C: mixrelay_time::MonotonicClock,
    E: crate::metadata_cache::MetadataEnvironment,
    F: crate::metadata_cache::MetadataFiles,
>(
    targets: Vec<PipeWireTarget>,
    resolver: &crate::metadata_cache::Resolver<C, E, F>,
) -> Vec<CapturableApplication> {
    let mut by_process = BTreeMap::new();
    for target in targets
        .into_iter()
        .filter(|target| target.media_class == "Stream/Output/Audio")
    {
        let Some(process_id) = target.process_id else {
            continue;
        };
        if by_process.contains_key(&process_id) {
            continue;
        }
        // Metadata does not choose capture identity: exact PipeWire application
        // ID or a verified kernel executable path provide persistent intent.
        let metadata = if by_process.len() < 64 {
            application_metadata::resolve(resolver, &target)
        } else {
            application_metadata::Metadata::default()
        };
        let stable_id = target
            .application_id
            .clone()
            .or_else(|| target.executable.clone())
            .unwrap_or_else(|| format!("pipewire:{}", target.serial));
        let selector = application_selector(&target, process_id);
        by_process
            .entry(process_id)
            .or_insert(CapturableApplication {
                bundle_id: stable_id,
                display_name: metadata.display_name.unwrap_or(target.display_name),
                icon_data_url: metadata.icon_data_url,
                process_id,
                selector,
            });
    }
    let mut applications = by_process.into_values().collect::<Vec<_>>();
    applications.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.process_id.cmp(&right.process_id))
    });
    applications
}

fn application_selector(target: &PipeWireTarget, process_id: u32) -> ApplicationSelector {
    if let Some(application_id) = target
        .application_id
        .as_ref()
        .filter(|id| !id.trim().is_empty())
    {
        return ApplicationSelector::BundleId {
            bundle_id: application_id.clone(),
            include_descendants: false,
        };
    }
    if let Some(path) =
        process_identity::verified_executable(process_id, target.executable.as_deref())
    {
        return ApplicationSelector::ExecutablePath { path };
    }
    ApplicationSelector::ProcessId {
        process_id,
        include_descendants: true,
    }
}

/// Returns physical and virtual `PipeWire` sources/sinks grouped by their device identity.
///
/// # Errors
///
/// Returns a backend error when the registry cannot be inspected.
pub fn devices() -> Result<Vec<AudioDeviceDescriptor>, HostAudioBackendError> {
    Ok(device_inventory(
        &targets()?,
        default_node_name("@DEFAULT_AUDIO_SOURCE@").as_deref(),
        default_node_name("@DEFAULT_AUDIO_SINK@").as_deref(),
    ))
}

fn device_inventory(
    targets: &[PipeWireTarget],
    default_input: Option<&str>,
    default_output: Option<&str>,
) -> Vec<AudioDeviceDescriptor> {
    let mut devices = BTreeMap::<String, AudioDeviceDescriptor>::new();
    for target in targets
        .iter()
        .filter(|target| matches!(target.media_class.as_str(), "Audio/Source" | "Audio/Sink"))
    {
        let Some(uid) = target.node_name.clone() else {
            continue;
        };
        let device = devices
            .entry(uid.clone())
            .or_insert_with(|| AudioDeviceDescriptor {
                uid,
                display_name: target.display_name.clone(),
                input_channels: 0,
                output_channels: 0,
                is_default_input: false,
                is_default_output: false,
            });
        if target.media_class == "Audio/Source" {
            device.input_channels = device.input_channels.max(target.channels);
            device.is_default_input |= default_input == Some(target.serial.as_str());
        } else {
            device.output_channels = device.output_channels.max(target.channels);
            device.is_default_output |= default_output == Some(target.serial.as_str());
        }
    }
    let mut devices = devices.into_values().collect::<Vec<_>>();
    devices.sort_unstable_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.uid.cmp(&right.uid))
    });
    devices
}

/// Resolve persistent selection against this session's exact node serial.
/// # Errors
/// Rejects missing, ambiguous, wrong-direction or undersized targets; never falls back.
pub fn resolve_device_target<'a>(
    targets: &'a [PipeWireTarget],
    selector: &DeviceSelector,
    direction: DeviceDirection,
    channels: u16,
) -> Result<&'a PipeWireTarget, HostAudioBackendError> {
    let default_input = matches!(selector, DeviceSelector::DefaultInput)
        .then(|| default_node_name("@DEFAULT_AUDIO_SOURCE@"))
        .flatten();
    let default_output = matches!(selector, DeviceSelector::DefaultOutput)
        .then(|| default_node_name("@DEFAULT_AUDIO_SINK@"))
        .flatten();
    let devices = device_inventory(targets, default_input.as_deref(), default_output.as_deref());
    let device = selector.resolve(&devices, direction, channels)?;
    let class = match direction {
        DeviceDirection::Input => "Audio/Source",
        DeviceDirection::Output => "Audio/Sink",
    };
    let mut matches = targets.iter().filter(|target| {
        target.node_name.as_deref() == Some(device.uid.as_str()) && target.media_class == class
    });
    let target = matches
        .next()
        .ok_or_else(|| backend_failure("device disappeared during selection".into()))?;
    if matches.next().is_some() {
        return Err(backend_failure("ambiguous PipeWire device identity".into()));
    }
    Ok(target)
}

fn parse_targets(values: &[Value]) -> Vec<PipeWireTarget> {
    values
        .iter()
        .filter(|value| {
            value.get("type").and_then(Value::as_str) == Some("PipeWire:Interface:Node")
        })
        .filter_map(|value| {
            let object_id = u32::try_from(value.get("id")?.as_u64()?).ok()?;
            let node_props = value.pointer("/info/props")?.as_object()?;
            // Some clients publish PID/application identity only on their Client
            // registry object. Node properties remain authoritative when present.
            let client = numeric_property(node_props, "client.id").and_then(|id| {
                values.iter().find(|entry| {
                    entry.get("id").and_then(Value::as_u64) == Some(u64::from(id))
                        && entry.get("type").and_then(Value::as_str)
                            == Some("PipeWire:Interface:Client")
                })
            });
            let mut inherited = client
                .and_then(|entry| entry.pointer("/info/props"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            inherited.extend(node_props.clone());
            let props = &inherited;
            let media_class = property(node_props, "media.class")?.to_owned();
            let serial = node_props.get("object.serial").and_then(|value| {
                value
                    .as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| value.as_str().map(str::to_owned))
            })?;
            // The owning client's name (often WirePlumber) is not the hardware
            // device name. Inherit application identity only for application UI.
            let display_name = (media_class == "Stream/Output/Audio")
                .then(|| property(props, "application.name"))
                .flatten()
                .or_else(|| property(node_props, "node.description"))
                .or_else(|| property(node_props, "node.nick"))
                .or_else(|| property(node_props, "node.name"))
                .unwrap_or("PipeWire audio")
                .to_owned();
            Some(PipeWireTarget {
                object_id,
                serial,
                node_name: property(node_props, "node.name").map(str::to_owned),
                process_id: numeric_property(props, "application.process.id"),
                application_id: property(props, "application.id").map(str::to_owned),
                executable: property(props, "application.process.binary").map(str::to_owned),
                icon_name: property(props, "application.icon-name").map(str::to_owned),
                display_name,
                media_class,
                channels: numeric_property(node_props, "audio.channels")
                    .or_else(|| channel_count_from_params(value))
                    .unwrap_or(0),
            })
        })
        .collect()
}

fn numeric_property(props: &serde_json::Map<String, Value>, key: &str) -> Option<u32> {
    let value = props.get(key)?;
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn property<'a>(props: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    props.get(key).and_then(Value::as_str)
}

fn channel_count_from_params(value: &Value) -> Option<u32> {
    let params = value.pointer("/info/params/EnumFormat")?.as_array()?;
    params.iter().find_map(find_channel_value)
}

fn find_channel_value(value: &Value) -> Option<u32> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            if key == "channels" {
                value
                    .as_u64()
                    .and_then(|channels| u32::try_from(channels).ok())
            } else {
                find_channel_value(value)
            }
        }),
        Value::Array(values) => values.iter().find_map(find_channel_value),
        _ => None,
    }
}

fn default_node_name(alias: &str) -> Option<String> {
    let output = command::output("wpctl", &["inspect", alias]).ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("object.serial = ")
                    .map(|value| value.trim_matches('"').to_owned())
            })
    })?
}

fn backend_failure(message: String) -> HostAudioBackendError {
    HostAudioBackendError::BackendFailure { message }
}

#[cfg(test)]
mod tests {
    struct Environment;
    impl crate::metadata_cache::MetadataEnvironment for Environment {
        fn snapshot(&self) -> crate::metadata_cache::Environment {
            crate::metadata_cache::Environment {
                roots: Vec::new(),
                locale: None,
                theme_name: None,
            }
        }
    }
    struct Files;
    impl crate::metadata_cache::MetadataFiles for Files {
        fn lookup(
            &self,
            _: &crate::metadata_cache::Key,
            _: &[std::path::PathBuf],
            _: Option<&str>,
        ) -> crate::metadata_cache::Metadata {
            crate::metadata_cache::Metadata::default()
        }
    }

    #[test]
    fn bounded_pipewire_cli_rejects_spent_deadline_and_unlisted_program_before_spawn() {
        let expired =
            super::bounded_pipewire_output("pw-dump", &[], std::time::Duration::ZERO).unwrap_err();
        assert_eq!(expired.kind(), std::io::ErrorKind::TimedOut);
        let unrelated = super::bounded_pipewire_output(
            "/bin/sleep",
            &["5"],
            std::time::Duration::from_millis(1),
        )
        .unwrap_err();
        assert_eq!(unrelated.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn native_numeric_properties_and_client_identity_are_not_lost() {
        let targets = super::parse_targets(&[
            serde_json::json!({"id":7,"type":"PipeWire:Interface:Client","info":{"props":{"application.process.id":1234,"application.name":"Player","application.id":"org.player","media.class":"Audio/Source","node.name":"stable-input","object.serial":777}}}),
            serde_json::json!({"id":8,"type":"PipeWire:Interface:Node","info":{"props":{"client.id":7,"object.serial":98765,"media.class":"Stream/Output/Audio","audio.channels":64}}}),
            serde_json::json!({"id":9,"type":"PipeWire:Interface:Node","info":{"props":{"client.id":"7","object.serial":"98766","media.class":"Audio/Source","node.name":"stable-input","application.process.id":"5678"}}}),
        ]);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].serial, "98765");
        assert_eq!(targets[0].process_id, Some(1234));
        assert_eq!(targets[0].application_id.as_deref(), Some("org.player"));
        assert_eq!(targets[0].channels, 64);
        assert_eq!(targets[1].process_id, Some(5678));
        assert_eq!(targets[1].channels, 0, "unknown width is not stereo");
        assert_eq!(targets[0].display_name, "Player");
        assert_eq!(targets[1].display_name, "stable-input");
        assert_eq!(
            targets[0].node_name, None,
            "client identity is not node identity"
        );
    }

    #[test]
    fn persistent_device_name_resolves_new_session_serial_without_fallback() {
        let mut targets = super::parse_targets(&[
            serde_json::json!({"id":8,"type":"PipeWire:Interface:Node","info":{"props":{
                "object.serial":1000,"media.class":"Audio/Source","node.name":"interface","audio.channels":64
            }}}),
        ]);
        let selector = mixrelay_audio_io::DeviceSelector::PersistentUid {
            uid: "interface".into(),
        };
        assert_eq!(
            super::resolve_device_target(
                &targets,
                &selector,
                mixrelay_audio_io::DeviceDirection::Input,
                64
            )
            .unwrap()
            .serial,
            "1000"
        );
        targets[0].serial = "2000".into();
        assert_eq!(
            super::resolve_device_target(
                &targets,
                &selector,
                mixrelay_audio_io::DeviceDirection::Input,
                64
            )
            .unwrap()
            .serial,
            "2000"
        );
        assert!(
            super::resolve_device_target(
                &targets,
                &selector,
                mixrelay_audio_io::DeviceDirection::Output,
                2
            )
            .is_err()
        );
        targets.push(targets[0].clone());
        assert!(
            super::resolve_device_target(
                &targets,
                &selector,
                mixrelay_audio_io::DeviceDirection::Input,
                2
            )
            .is_err()
        );
        assert!(
            super::resolve_device_target(
                &[],
                &selector,
                mixrelay_audio_io::DeviceDirection::Input,
                2
            )
            .is_err()
        );
    }
    #[test]
    fn concurrent_registry_arrays_preserve_identity_and_apply_removals() {
        let objects = super::parse_registry_dump(
            br#"[{"id":1,"type":"Node","info":{"props":{"name":"] ["}}},{"id":2}]
            [{"id":1,"info":{"props":{"channels":64}}},{"id":2,"info":null}]"#,
        )
        .unwrap();
        assert_eq!(
            objects,
            vec![
                serde_json::json!({"id":1,"type":"Node","info":{"props":{"name":"] [","channels":64}}})
            ]
        );
        for bad in [b"".as_slice(), b"[] invalid", b"[{}]", b"[{", b"{}"] {
            assert!(super::parse_registry_dump(bad).is_err());
        }
    }
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_application_and_device_properties() {
        let targets = parse_targets(&[
            json!({
                "id": 42,
                "type": "PipeWire:Interface:Node",
                "info": { "props": {
                    "object.serial": "9001",
                    "media.class": "Stream/Output/Audio",
                    "application.process.id": "123",
                    "application.id": "org.example.Player",
                    "application.icon-name": "player",
                    "application.name": "Player",
                    "audio.channels": "6"
                }}
            }),
            json!({
                "id": 43,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "object.serial": "9002",
                        "media.class": "Audio/Source",
                        "node.description": "Interface"
                    },
                    "params": { "EnumFormat": [{ "format": { "channels": 8 } }] }
                }
            }),
        ]);
        assert_eq!(targets[0].process_id, Some(123));
        assert_eq!(targets[0].channels, 6);
        assert_eq!(targets[0].icon_name.as_deref(), Some("player"));
        assert_eq!(targets[1].channels, 8);
    }

    #[test]
    fn application_id_provides_persistent_intent_without_losing_distinct_instances() {
        let values = [101, 101, 202].map(|pid| {
            json!({
                "id": pid, "type": "PipeWire:Interface:Node", "info": {"props": {
                    "object.serial": pid.to_string(), "media.class": "Stream/Output/Audio",
                    "application.process.id": pid, "application.id": "org.mixrelay.fixture.missing",
                    "application.name": "Fixture App"
                }}
            })
        });
        let clock = mixrelay_time::ManualClock::default();
        let resolver = crate::metadata_cache::Resolver::new(clock, Environment, Files);
        let apps = application_inventory(parse_targets(&values), &resolver);
        assert_eq!(apps.len(), 2);
        assert!(
            apps.iter()
                .all(|app| app.bundle_id == "org.mixrelay.fixture.missing"
                    && app.display_name == "Fixture App"
                    && app.icon_data_url.is_none())
        );
        assert_eq!(
            apps[0].selector,
            ApplicationSelector::BundleId {
                bundle_id: "org.mixrelay.fixture.missing".into(),
                include_descendants: false,
            }
        );
        assert_eq!(apps[1].process_id, 202);
    }
}
