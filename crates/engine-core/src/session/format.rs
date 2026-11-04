//! Stages saved preferences and serializes session format changes.

use super::{
    AdapterRuntimeStatus, Ordering, RenderFormat, SessionFormat, SessionFormatBackend,
    SessionPreferences, StagedSessionPreferences, pack_format, unpack_format,
};

impl AdapterRuntimeStatus {
    /// Startup/control thread only, before creating the DSP worker. Bad or unsupported
    /// preferences never prevent the local control service from starting.
    pub fn restore_preferences(
        &mut self,
        store: Result<std::sync::Arc<dyn SessionPreferences>, String>,
    ) -> RenderFormat {
        let fallback = unpack_format(self.active_format.load(Ordering::Relaxed));
        let loaded = store
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|store| store.load().map_err(|e| e.to_string()));
        self.preferences = Some(store);
        let requested = match loaded {
            Ok(Some(format)) => RenderFormat {
                sample_rate: format.sample_rate,
                block_frames: format.block_frames,
            },
            Ok(None) => return fallback,
            Err(error) => {
                self.preferences_error.store(true, Ordering::Release);
                tracing::warn!(%error, "session preferences not restored");
                return fallback;
            }
        };
        self.restore_saved_format(requested, fallback)
    }

    pub(super) fn restore_saved_format(
        &self,
        requested: RenderFormat,
        fallback: RenderFormat,
    ) -> RenderFormat {
        self.preferred_format
            .store(pack_format(requested), Ordering::Release);
        if self.native.follows_rate() && self.supports_routing_format(requested) {
            return self.restore_followed_rate(requested, fallback);
        }
        if !self.supports_format(requested) {
            self.activate_format(fallback);
            self.preferences_error.store(true, Ordering::Release);
            tracing::warn!(
                ?requested,
                "stored session format is unsupported by this adapter"
            );
            return fallback;
        }
        self.activate_format(requested);
        requested
    }

    /// Before any worker starts, restore the client buffer preference but let the
    /// device retain its own nominal rate. An unpublished device is unavailable,
    /// not a failed settings write; later observation or hardware routing resolves it.
    pub(super) fn restore_followed_rate(
        &self,
        saved: RenderFormat,
        fallback: RenderFormat,
    ) -> RenderFormat {
        let observed = self.external_sample_rate().filter(|rate| {
            self.profile
                .capabilities
                .supported_sample_rates
                .contains(rate)
        });
        let actual = RenderFormat {
            sample_rate: observed.unwrap_or(fallback.sample_rate),
            block_frames: saved.block_frames,
        };
        if observed.is_some() && actual != saved {
            let persisted = self.stage_preferences(actual).and_then(|staged| {
                staged.map_or(Ok(()), |staged| {
                    staged.commit().map_err(|error| error.to_string())
                })
            });
            self.preferences_error
                .store(persisted.is_err(), Ordering::Release);
            self.preferred_format
                .store(pack_format(actual), Ordering::Release);
        }
        self.activate_format(actual);
        actual
    }

    pub(super) fn supports_format(&self, format: RenderFormat) -> bool {
        self.supported_sample_rates().contains(&format.sample_rate)
            && self
                .profile
                .capabilities
                .supported_block_frames
                .contains(&format.block_frames)
    }

    pub(super) fn supports_routing_format(&self, format: RenderFormat) -> bool {
        self.profile
            .capabilities
            .supported_sample_rates
            .contains(&format.sample_rate)
            && self
                .profile
                .capabilities
                .supported_block_frames
                .contains(&format.block_frames)
    }

    pub(super) fn stage_preferences(
        &self,
        format: RenderFormat,
    ) -> Result<Option<Box<dyn StagedSessionPreferences>>, String> {
        self.preferences
            .as_ref()
            .map(|store| {
                store
                    .as_ref()
                    .map_err(Clone::clone)?
                    .stage(SessionFormat {
                        sample_rate: format.sample_rate,
                        block_frames: format.block_frames,
                    })
                    .map_err(|error| error.to_string())
            })
            .transpose()
    }

    /// Follow an already-running routing clock without rewriting an unrelated
    /// virtual endpoint or its saved nominal rate. The caller must publish the
    /// newly prepared DSP plan before resuming the processing owner.
    /// # Errors
    /// Rejects unsupported product geometry or a failed processing suspension.
    pub fn adopt_routing_clock(
        &self,
        requested: RenderFormat,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        if !self.supports_routing_format(requested) {
            return Err(
                mixrelay_audio_io::HostAudioBackendError::CapabilityUnavailable {
                    capability: "routing clock format".into(),
                },
            );
        }
        self.pause_at_boundary()?;
        self.observe_routing_clock(true, Some(requested.sample_rate));
        self.failed.store(false, Ordering::Release);
        Ok(())
    }

    /// Saves a user-selected DSP buffer while retaining the virtual endpoint's
    /// own nominal-rate preference. Native hardware buffer negotiation remains
    /// the prepared device transaction's responsibility; no HAL setter runs here.
    /// # Errors
    /// Rejects unsupported geometry, preference staging/commit failures, or a
    /// processing owner that does not acknowledge suspension.
    pub fn reconfigure_routing_buffer(
        &self,
        requested: RenderFormat,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        use mixrelay_audio_io::HostAudioBackendError;
        if !self.supports_routing_format(requested) {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "routing buffer format".into(),
            });
        }
        let saved = RenderFormat {
            sample_rate: self.preferred_format().map_or_else(
                || {
                    self.observed_endpoint_rate()
                        .unwrap_or(self.profile.initial_format.sample_rate)
                },
                |format| format.sample_rate,
            ),
            block_frames: requested.block_frames,
        };
        let staged = self.stage_preferences(saved).map_err(|message| {
            self.preferences_error.store(true, Ordering::Release);
            HostAudioBackendError::BackendFailure { message }
        })?;
        self.pause_at_boundary()?;
        if let Some(staged) = staged {
            staged.commit().map_err(|error| {
                self.preferences_error.store(true, Ordering::Release);
                HostAudioBackendError::BackendFailure {
                    message: error.to_string(),
                }
            })?;
        }
        if self.preferences.is_some() {
            self.preferred_format
                .store(pack_format(saved), Ordering::Release);
        }
        self.observe_routing_clock(true, Some(requested.sample_rate));
        self.preferences_error.store(false, Ordering::Release);
        self.failed.store(false, Ordering::Release);
        Ok(())
    }

    pub(super) fn preferred_format(&self) -> Option<SessionFormat> {
        let value = self.preferred_format.load(Ordering::Acquire);
        (value != 0).then(|| {
            let format = unpack_format(value);
            SessionFormat {
                sample_rate: format.sample_rate,
                block_frames: format.block_frames,
            }
        })
    }
}
impl SessionFormatBackend for AdapterRuntimeStatus {
    fn external_sample_rate(&self) -> Option<u32> {
        let rate = self.native.read_rate();
        if self.native.follows_rate() {
            self.observe_device_rate(rate);
        }
        rate
    }
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        self.external_sample_rate().map(|sample_rate| RenderFormat {
            sample_rate,
            ..current
        })
    }
    fn adopt_external_format(
        &self,
        requested: RenderFormat,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        use mixrelay_audio_io::HostAudioBackendError;
        if !self.supports_format(requested) || !self.native.follows_rate() {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "external format following".into(),
            });
        }
        if self.external_format(requested) != Some(requested) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "external native format changed during preparation".into(),
            });
        }
        // The device has already changed. Storage failures are warnings, not a reason
        // to keep processing PCM at the wrong rate. Preserve the last good file.
        let staged = self.stage_preferences(requested);
        self.pause_at_boundary()?;
        if self.external_format(requested) != Some(requested) {
            return Err(HostAudioBackendError::BackendFailure {
                message: "external native format changed during suspension".into(),
            });
        }
        let saved = staged.and_then(|staged| {
            staged.map_or(Ok(()), |staged| {
                staged.commit().map_err(|error| error.to_string())
            })
        });
        self.preferences_error
            .store(saved.is_err(), Ordering::Release);
        if let Err(error) = saved {
            tracing::warn!(%error, ?requested, "externally accepted format was not saved");
        }
        if self.preferences.is_some() {
            self.preferred_format
                .store(pack_format(requested), Ordering::Release);
        }
        // The caller publishes the prepared DSP plan before resuming; the saved
        // last-accepted format prevents a restart from undoing the DAW's change.
        self.failed.store(false, Ordering::Release);
        Ok(())
    }
    fn reconfigure(
        &self,
        requested: RenderFormat,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        use mixrelay_audio_io::HostAudioBackendError;
        if !self.supports_format(requested) || !self.profile.capabilities.can_reconfigure_live {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "requested native session format".into(),
            });
        }
        if !self.native.can_reconfigure() {
            return Err(HostAudioBackendError::CapabilityUnavailable {
                capability: "live format reconfiguration".into(),
            });
        }
        // Serialize and sync the new preference before suspending audio; the graph
        // format changes when the caller publishes the prepared plan, not here.
        let staged = self.stage_preferences(requested).map_err(|message| {
            self.preferences_error.store(true, Ordering::Release);
            HostAudioBackendError::BackendFailure { message }
        })?;
        self.pause_at_boundary()?;
        if let Some(staged) = staged {
            staged.commit().map_err(|error| {
                self.preferences_error.store(true, Ordering::Release);
                HostAudioBackendError::BackendFailure {
                    message: format!("session preferences could not be committed: {error}"),
                }
            })?;
        }
        if self.preferences.is_some() {
            self.preferred_format
                .store(pack_format(requested), Ordering::Release);
        }
        self.preferences_error.store(false, Ordering::Release);
        self.failed.store(false, Ordering::Release);
        Ok(())
    }
}
