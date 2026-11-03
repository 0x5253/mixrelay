//! Which apps are playing into or recording from each `MixRelay` virtual device, from
//! `CoreAudio` process objects. Helper processes (a browser's audio helper) are shown as
//! the app they belong to, and `MixRelay`'s own processes (the engine) are left out.
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, PoisonError},
};

use mixrelay_audio_io::CapturableApplication;
use mixrelay_process_tap::DeviceUse;
use serde::Serialize;

/// `MixRelay` virtual devices are named by this UID prefix.
const ENDPOINT_PREFIX: &str = "io.mixrelay.audio.endpoint.";
/// `MixRelay`'s own processes use every device; they are not "apps using" it.
const OWN_PREFIX: &str = "io.mixrelay.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceAppActivity {
    pub(crate) bundle_id: String,
    pub(crate) display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon_data_url: Option<String>,
    pub(crate) process_id: i32,
    pub(crate) playing: bool,
    pub(crate) recording: bool,
}

/// Display name and icon of a running app.
type Identity = (String, Option<String>);
/// Running apps by bundle ID.
type Identities = HashMap<String, Identity>;

/// One editor owns its app-identity cache. Construction does not query the OS.
#[derive(Debug)]
pub(crate) struct DeviceActivityCache {
    identities: Mutex<Identities>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ActivityReadFailure {
    #[error("{0}")]
    DeviceUses(#[source] mixrelay_process_tap::TapError),
    #[error("application inventory unavailable: {0}")]
    Applications(String),
}

trait ActivitySource {
    fn device_uses(&self) -> Result<Vec<DeviceUse>, ActivityReadFailure>;
    fn applications(&self) -> Result<Vec<CapturableApplication>, ActivityReadFailure>;
}

struct CoreAudioActivity;
impl ActivitySource for CoreAudioActivity {
    fn device_uses(&self) -> Result<Vec<DeviceUse>, ActivityReadFailure> {
        mixrelay_process_tap::device_uses().map_err(ActivityReadFailure::DeviceUses)
    }
    fn applications(&self) -> Result<Vec<CapturableApplication>, ActivityReadFailure> {
        super::list_capturable_applications().map_err(ActivityReadFailure::Applications)
    }
}

fn identities_from(applications: Vec<CapturableApplication>) -> Identities {
    applications
        .into_iter()
        .map(|app| (app.bundle_id, (app.display_name, app.icon_data_url)))
        .collect()
}

/// The running app a bundle belongs to: itself, or the app whose bundle ID prefixes it
/// (`com.google.Chrome.helper` belongs to `com.google.Chrome`), longest match first.
fn owner<'a>(bundle_id: &str, identities: &'a Identities) -> Option<(&'a str, &'a Identity)> {
    identities
        .iter()
        .filter(|(app, _)| {
            bundle_id == app.as_str()
                || bundle_id
                    .strip_prefix(app.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        })
        .max_by_key(|(app, _)| app.len())
        .map(|(app, identity)| (app.as_str(), identity))
}

/// Groups device uses by `MixRelay` device, one entry per app (helpers merged into their
/// app, flags combined), in a stable order.
fn group(
    uses: Vec<DeviceUse>,
    identities: &Identities,
) -> BTreeMap<String, Vec<DeviceAppActivity>> {
    let mut devices: BTreeMap<String, Vec<DeviceAppActivity>> = BTreeMap::new();
    for entry in uses {
        if !entry.device_uid.starts_with(ENDPOINT_PREFIX) || entry.bundle_id.starts_with(OWN_PREFIX)
        {
            continue;
        }
        let (bundle_id, display_name, icon_data_url) = match owner(&entry.bundle_id, identities) {
            Some((app, (name, icon))) => (app.to_owned(), name.clone(), icon.clone()),
            None => (entry.bundle_id.clone(), entry.bundle_id.clone(), None),
        };
        let apps = devices.entry(entry.device_uid).or_default();
        match apps.iter_mut().find(|app| app.bundle_id == bundle_id) {
            Some(app) => {
                app.playing |= entry.playing;
                app.recording |= entry.recording;
            }
            None => apps.push(DeviceAppActivity {
                bundle_id,
                display_name,
                icon_data_url,
                process_id: entry.process_id,
                playing: entry.playing,
                recording: entry.recording,
            }),
        }
    }
    for apps in devices.values_mut() {
        apps.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then(left.bundle_id.cmp(&right.bundle_id))
        });
    }
    devices
}

impl DeviceActivityCache {
    pub(crate) fn new() -> Self {
        Self {
            identities: Mutex::new(Identities::new()),
        }
    }

    pub(super) fn activity(
        &self,
    ) -> Result<BTreeMap<String, Vec<DeviceAppActivity>>, ActivityReadFailure> {
        self.observe(&CoreAudioActivity)
    }

    fn observe(
        &self,
        source: &impl ActivitySource,
    ) -> Result<BTreeMap<String, Vec<DeviceAppActivity>>, ActivityReadFailure> {
        let uses = source.device_uses()?;
        let mut cached = self
            .identities
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let unknown = uses.iter().any(|entry| {
            entry.device_uid.starts_with(ENDPOINT_PREFIX)
                && !entry.bundle_id.starts_with(OWN_PREFIX)
                && owner(&entry.bundle_id, &cached).is_none()
        });
        if unknown {
            *cached = identities_from(source.applications()?);
        }
        Ok(group(uses, &cached))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Inventory {
        uses: RefCell<Vec<DeviceUse>>,
        apps: RefCell<Vec<CapturableApplication>>,
        uses_failure: Cell<Option<mixrelay_process_tap::TapError>>,
        apps_failure: Cell<bool>,
        uses_reads: Cell<usize>,
        apps_reads: Cell<usize>,
    }

    impl ActivitySource for Inventory {
        fn device_uses(&self) -> Result<Vec<DeviceUse>, ActivityReadFailure> {
            self.uses_reads.set(self.uses_reads.get() + 1);
            if let Some(error) = self.uses_failure.get() {
                return Err(ActivityReadFailure::DeviceUses(error));
            }
            Ok(self.uses.borrow().clone())
        }
        fn applications(&self) -> Result<Vec<CapturableApplication>, ActivityReadFailure> {
            self.apps_reads.set(self.apps_reads.get() + 1);
            if self.apps_failure.get() {
                return Err(ActivityReadFailure::Applications(
                    "fixture unavailable".into(),
                ));
            }
            Ok(self.apps.borrow().clone())
        }
    }

    fn application(bundle: &str, name: &str) -> CapturableApplication {
        CapturableApplication {
            bundle_id: bundle.into(),
            display_name: name.into(),
            icon_data_url: Some("icon".into()),
            process_id: 7,
            selector: mixrelay_audio_io::ApplicationSelector::BundleId {
                bundle_id: bundle.into(),
                include_descendants: true,
            },
        }
    }

    fn device_use(bundle_id: &str, device: &str, playing: bool, recording: bool) -> DeviceUse {
        DeviceUse {
            process_id: 7,
            bundle_id: bundle_id.to_owned(),
            device_uid: format!("{ENDPOINT_PREFIX}{device}"),
            playing,
            recording,
        }
    }

    #[test]
    fn helpers_join_their_app_and_mixrelay_processes_are_left_out() {
        let identities: Identities = [
            (
                "com.google.Chrome".to_owned(),
                ("Chrome".to_owned(), Some("icon".to_owned())),
            ),
            ("com.google".to_owned(), ("Too broad".to_owned(), None)),
            ("com.apple.Music".to_owned(), ("Music".to_owned(), None)),
        ]
        .into_iter()
        .collect();
        let uses = vec![
            device_use("com.google.Chrome.helper", "a", true, false),
            device_use("com.google.Chrome", "a", false, true),
            device_use("com.apple.Music", "a", true, false),
            device_use("io.mixrelay.audio.engine", "a", true, true),
            device_use("org.unknown.Tool", "b", false, true),
            DeviceUse {
                device_uid: "BuiltInSpeakerDevice".to_owned(),
                ..device_use("com.apple.Music", "", true, false)
            },
        ];
        let devices = group(uses, &identities);
        let app =
            |bundle: &str, name: &str, icon: Option<&str>, playing, recording| DeviceAppActivity {
                bundle_id: bundle.to_owned(),
                display_name: name.to_owned(),
                icon_data_url: icon.map(str::to_owned),
                process_id: 7,
                playing,
                recording,
            };
        assert_eq!(
            devices[&format!("{ENDPOINT_PREFIX}a")],
            [
                app("com.google.Chrome", "Chrome", Some("icon"), true, true),
                app("com.apple.Music", "Music", None, true, false),
            ]
        );
        assert_eq!(
            devices[&format!("{ENDPOINT_PREFIX}b")],
            [app(
                "org.unknown.Tool",
                "org.unknown.Tool",
                None,
                false,
                true
            )]
        );
        assert_eq!(devices.len(), 2);
    }

    #[test]
    fn known_apps_reuse_identity_but_each_read_observes_current_device_use() {
        let source = Inventory::default();
        source
            .uses
            .borrow_mut()
            .push(device_use("org.example.App.helper", "a", true, false));
        source
            .apps
            .borrow_mut()
            .push(application("org.example.App", "App"));
        let cache = DeviceActivityCache::new();
        let first = cache.observe(&source).unwrap();
        assert_eq!(first[&format!("{ENDPOINT_PREFIX}a")][0].display_name, "App");
        source.uses.borrow_mut()[0].recording = true;
        assert!(cache.observe(&source).unwrap()[&format!("{ENDPOINT_PREFIX}a")][0].recording);
        assert_eq!(source.uses_reads.get(), 2);
        assert_eq!(source.apps_reads.get(), 1);
        source
            .uses
            .borrow_mut()
            .push(device_use("org.example.Other", "b", true, false));
        source
            .apps
            .borrow_mut()
            .push(application("org.example.Other", "Other"));
        assert_eq!(
            cache.observe(&source).unwrap()[&format!("{ENDPOINT_PREFIX}b")][0].display_name,
            "Other"
        );
        assert_eq!(source.apps_reads.get(), 2);
        assert_eq!(
            DeviceActivityCache::new().observe(&source).unwrap().len(),
            2
        );
        assert_eq!(source.apps_reads.get(), 3);
    }

    #[test]
    fn unrelated_devices_and_own_processes_do_not_query_application_identity() {
        let source = Inventory::default();
        source.apps_failure.set(true);
        source.uses.borrow_mut().extend([
            DeviceUse {
                device_uid: "BuiltInSpeakerDevice".into(),
                ..device_use("org.example.Unknown", "a", true, false)
            },
            device_use("io.mixrelay.audio.engine", "a", true, true),
        ]);
        assert!(
            DeviceActivityCache::new()
                .observe(&source)
                .unwrap()
                .is_empty()
        );
        assert_eq!(source.apps_reads.get(), 0);
    }

    #[test]
    fn failed_native_query_preserves_status_and_never_looks_up_app_identity() {
        let source = Inventory::default();
        let error = mixrelay_process_tap::TapError {
            code: 4,
            os_status: -17,
        };
        source.uses_failure.set(Some(error));
        assert_eq!(
            DeviceActivityCache::new().observe(&source),
            Err(ActivityReadFailure::DeviceUses(error))
        );
        assert_eq!(source.apps_reads.get(), 0);
    }

    #[test]
    fn failed_application_refresh_keeps_last_confirmed_identity() {
        let source = Inventory::default();
        source
            .uses
            .borrow_mut()
            .push(device_use("org.example.App", "a", true, false));
        source
            .apps
            .borrow_mut()
            .push(application("org.example.App", "Original"));
        let cache = DeviceActivityCache::new();
        cache.observe(&source).unwrap();
        source
            .uses
            .borrow_mut()
            .push(device_use("org.example.Unknown", "b", true, false));
        source.apps_failure.set(true);
        assert!(matches!(
            cache.observe(&source),
            Err(ActivityReadFailure::Applications(_))
        ));
        source.uses.borrow_mut().pop();
        source.apps.borrow_mut().clear();
        let observed = cache.observe(&source).unwrap();
        assert_eq!(
            observed[&format!("{ENDPOINT_PREFIX}a")][0].display_name,
            "Original"
        );
        assert_eq!(source.apps_reads.get(), 2);
    }
}
