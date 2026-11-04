//! What the status menu knows and which actions it admits right now.
use std::collections::HashMap;

use mixrelay_control_protocol::EnginePowerStatus;
use mixrelay_desktop_core::{devices::SettingAction, updates::UpdateState};
use mixrelay_driver_core::DriverSnapshot;

/// The row shown under Check for Updates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UpdateResult {
    Current,
    NotPublished,
    CheckFailed,
    Download { version: String, url: String },
    DownloadFailed,
}

impl UpdateResult {
    /// Selects one row from validated catalog data; routine automatic results stay quiet.
    pub(super) fn after_check(
        state: UpdateState,
        download: Option<(&str, &str)>,
        manual: bool,
    ) -> Option<Self> {
        match state {
            UpdateState::Available | UpdateState::Published => download
                .map(|(version, url)| Self::Download {
                    version: version.to_owned(),
                    url: url.to_owned(),
                })
                .or_else(|| manual.then_some(Self::CheckFailed)),
            UpdateState::Current => manual.then_some(Self::Current),
            UpdateState::NotPublished => manual.then_some(Self::NotPublished),
            UpdateState::Unavailable => manual.then_some(Self::CheckFailed),
        }
    }
}

#[derive(Default)]
pub(super) struct MenuState {
    pub(super) locale: String,
    pub(super) snapshot: Option<DriverSnapshot>,
    /// A Driver readback or setting change is in flight.
    pub(super) busy: bool,
    pub(super) stopping: bool,
    pub(super) error: Option<String>,
    pub(super) update_result: Option<UpdateResult>,
    /// An update check or download link is in flight.
    pub(super) updates_busy: bool,
    /// Renders so far; choice IDs from an older menu never match a new action.
    pub(super) generation: u64,
    pub(super) actions: HashMap<String, SettingAction>,
    pub(super) audio_power: Option<EnginePowerStatus>,
    /// The requested audio state while a pause or resume is in flight.
    pub(super) audio_change: Option<bool>,
}

impl MenuState {
    /// No Driver request, audio change or stop is in progress.
    pub(super) fn driver_idle(&self) -> bool {
        !self.busy && self.audio_change.is_none() && !self.stopping
    }

    /// Device settings accept changes only from a ready Driver while nothing is pending.
    pub(super) fn devices_editable(&self) -> bool {
        self.driver_idle()
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.state == mixrelay_driver_core::DriverState::Ready)
    }

    /// Audio is changing, by this menu or by the engine itself.
    pub(super) fn audio_pending(&self) -> bool {
        self.audio_change.is_some() || self.audio_power.is_some_and(|power| power.transitioning)
    }

    pub(super) fn can_toggle_audio(&self) -> bool {
        self.audio_power.is_some() && !self.audio_pending() && !self.busy && !self.stopping
    }

    /// Starts a Driver readback or setting change. Returns `false` when one cannot start.
    pub(super) fn begin_driver_request(&mut self) -> bool {
        if !self.driver_idle() {
            return false;
        }
        self.busy = true;
        self.error = None;
        true
    }

    pub(super) fn finish_driver_request(&mut self, result: Result<DriverSnapshot, String>) {
        self.busy = false;
        match result {
            Ok(snapshot) => {
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => {
                tracing::error!(%error, "status menu action failed");
                self.error = Some(error);
            }
        }
    }

    /// Starts pausing or resuming. Returns the requested enabled state.
    pub(super) fn begin_audio_change(&mut self) -> Option<bool> {
        if !self.can_toggle_audio() {
            return None;
        }
        let enabled = !self.audio_power?.enabled;
        self.audio_change = Some(enabled);
        Some(enabled)
    }

    /// Records observed engine power. Returns whether it changed.
    pub(super) fn observe_audio_power(&mut self, power: Option<EnginePowerStatus>) -> bool {
        let changed = self.audio_power != power;
        self.audio_power = power;
        changed
    }

    /// Returns whether the stopping state changed.
    pub(super) fn set_stopping(&mut self, stopping: bool) -> bool {
        let changed = self.stopping != stopping;
        self.stopping = stopping;
        changed
    }

    /// Starts an update check. Returns `false` while another update request runs.
    pub(super) fn begin_update_check(&mut self) -> bool {
        if self.updates_busy {
            return false;
        }
        self.updates_busy = true;
        true
    }

    pub(super) fn finish_update_check(&mut self, result: Option<UpdateResult>) {
        self.update_result = result;
        self.updates_busy = false;
    }

    /// Starts opening the offered download. Returns its URL, or `None` when nothing is
    /// offered or another update request runs.
    pub(super) fn begin_download(&mut self) -> Option<String> {
        if self.updates_busy {
            return None;
        }
        let Some(UpdateResult::Download { url, .. }) = &self.update_result else {
            return None;
        };
        let url = url.clone();
        self.updates_busy = true;
        Some(url)
    }

    pub(super) fn finish_download(&mut self, opened: bool) {
        self.updates_busy = false;
        if !opened {
            self.update_result = Some(UpdateResult::DownloadFailed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(enabled: bool, transitioning: bool) -> EnginePowerStatus {
        EnginePowerStatus {
            enabled,
            transitioning,
            unsaved: false,
            pause_reason: None,
        }
    }

    #[test]
    fn routine_update_results_are_shown_only_after_manual_checks() {
        for (state, result) in [
            (UpdateState::Current, UpdateResult::Current),
            (UpdateState::NotPublished, UpdateResult::NotPublished),
            (UpdateState::Unavailable, UpdateResult::CheckFailed),
        ] {
            assert_eq!(UpdateResult::after_check(state, None, false), None);
            assert_eq!(UpdateResult::after_check(state, None, true), Some(result));
        }
    }

    #[test]
    fn available_downloads_have_one_action_for_manual_and_automatic_checks() {
        let download = Some(("0.2.0", "https://example.com/driver.pkg"));
        let expected = UpdateResult::Download {
            version: "0.2.0".into(),
            url: "https://example.com/driver.pkg".into(),
        };
        for state in [UpdateState::Available, UpdateState::Published] {
            for manual in [false, true] {
                assert_eq!(
                    UpdateResult::after_check(state, download, manual),
                    Some(expected.clone())
                );
            }
            assert_eq!(UpdateResult::after_check(state, None, false), None);
            assert_eq!(
                UpdateResult::after_check(state, None, true),
                Some(UpdateResult::CheckFailed)
            );
        }
    }

    #[test]
    fn one_driver_request_runs_at_a_time_and_none_starts_during_audio_changes_or_quit() {
        let mut state = MenuState {
            error: Some("previous".into()),
            ..MenuState::default()
        };
        assert!(state.begin_driver_request());
        assert_eq!(state.error, None);
        assert!(!state.begin_driver_request());
        state.finish_driver_request(Err("failed".into()));
        assert_eq!(state.error.as_deref(), Some("failed"));
        assert!(!state.busy);

        state.audio_change = Some(false);
        assert!(!state.begin_driver_request());
        state.audio_change = None;
        state.set_stopping(true);
        assert!(!state.begin_driver_request());
    }

    #[test]
    fn pause_toggles_the_observed_power_once_and_waits_for_every_pending_change() {
        let mut state = MenuState::default();
        assert_eq!(state.begin_audio_change(), None, "power not observed yet");
        state.observe_audio_power(Some(power(true, false)));
        assert_eq!(state.begin_audio_change(), Some(false));
        assert_eq!(state.begin_audio_change(), None, "already changing");

        state.audio_change = None;
        state.observe_audio_power(Some(power(false, true)));
        assert_eq!(state.begin_audio_change(), None, "engine is transitioning");
        state.observe_audio_power(Some(power(false, false)));
        state.busy = true;
        assert_eq!(state.begin_audio_change(), None, "driver request in flight");
        state.busy = false;
        state.stopping = true;
        assert_eq!(state.begin_audio_change(), None, "quitting");
        state.stopping = false;
        assert_eq!(state.begin_audio_change(), Some(true));
    }

    #[test]
    fn stopping_and_power_observations_report_only_real_changes() {
        let mut state = MenuState::default();
        assert!(state.set_stopping(true));
        assert!(!state.set_stopping(true));
        assert!(state.set_stopping(false));
        assert!(state.observe_audio_power(Some(power(true, false))));
        assert!(!state.observe_audio_power(Some(power(true, false))));
        assert!(state.observe_audio_power(None));
    }

    #[test]
    fn update_requests_are_serialized_and_a_failed_link_replaces_the_download_row() {
        let mut state = MenuState::default();
        assert_eq!(state.begin_download(), None, "nothing offered");
        assert!(state.begin_update_check());
        assert!(!state.begin_update_check());
        state.finish_update_check(UpdateResult::after_check(
            UpdateState::Available,
            Some(("0.2.0", "https://example.com/a.pkg")),
            false,
        ));
        assert_eq!(
            state.begin_download().as_deref(),
            Some("https://example.com/a.pkg")
        );
        assert_eq!(state.begin_download(), None, "link already opening");
        assert!(!state.begin_update_check());
        state.finish_download(true);
        assert!(matches!(
            state.update_result,
            Some(UpdateResult::Download { .. })
        ));
        assert!(state.begin_download().is_some());
        state.finish_download(false);
        assert_eq!(state.update_result, Some(UpdateResult::DownloadFailed));
        assert_eq!(state.begin_download(), None);
    }
}
