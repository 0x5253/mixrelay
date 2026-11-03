//! Stages saved preferences and serializes native session format transactions.

use super::{
    AdapterRuntimeStatus, Ordering, RenderFormat, SessionFormat, SessionFormatBackend,
    SessionPreferences, StagedSessionPreferences, pack_format, unpack_format,
};

impl AdapterRuntimeStatus {
    /// Startup/control thread only, before creating the DSP worker. Bad preferences or
    /// failed native negotiation never prevent the local control service from starting.
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
        if self.native.follows_rate()
            && !self.native.has_pair_control()
            && self.supports_routing_format(requested)
        {
            return self.restore_followed_rate(requested, fallback);
        }
        // A rate-only own endpoint does not own the DSP buffer preference.
        // Retain that validated buffer when hardware can run without this HAL.
        let independent_buffer = self.native.has_rate_control() && !self.native.has_pair_control();
        let fallback = if independent_buffer && self.supports_routing_format(requested) {
            RenderFormat {
                block_frames: requested.block_frames,
                ..fallback
            }
        } else {
            fallback
        };
        self.activate_format(fallback);
        if !self.supports_format(requested) {
            if independent_buffer && self.supports_routing_format(requested) {
                self.restore_failed.store(true, Ordering::Release);
            } else {
                self.preferences_error.store(true, Ordering::Release);
            }
            tracing::warn!(
                ?requested,
                "stored session format is unsupported by this adapter"
            );
            return fallback;
        }
        if let Err(error) = self.restore_native_format(requested, fallback) {
            self.restore_failed.store(true, Ordering::Release);
            tracing::warn!(%error, ?requested, "stored native format could not be restored");
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

    pub(super) fn write_native_format(&self, requested: RenderFormat) -> Result<(), String> {
        self.native
            .write(requested, |actual| self.observe_device_format(actual))
    }

    pub(super) fn native_rollback_format(
        &self,
        fallback: RenderFormat,
    ) -> Result<RenderFormat, String> {
        self.native.rollback_format(fallback)
    }

    pub(super) fn restore_native_format(
        &self,
        requested: RenderFormat,
        fallback: RenderFormat,
    ) -> Result<(), String> {
        let previous = self.native_rollback_format(fallback)?;
        if let Err(error) = self.write_native_format(requested) {
            if let Err(rollback) = self.write_native_format(previous) {
                self.failed.store(true, Ordering::Release);
                return Err(format!("{error}; rollback failed: {rollback}"));
            }
            return Err(error);
        }
        Ok(())
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
                    self.endpoint_format_settings()
                        .0
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
        if self.native.follows_pair() {
            let format = self.native.read_pair();
            self.observe_device_format(format);
            return format.map(|format| format.sample_rate);
        }
        let rate = self.native.read_rate();
        if self.native.follows_rate() {
            self.observe_device_rate(rate);
        }
        rate
    }
    fn external_format(&self, current: RenderFormat) -> Option<RenderFormat> {
        if self.native.follows_pair() {
            let format = self.native.read_pair();
            self.observe_device_format(format);
            format
        } else {
            self.external_sample_rate().map(|sample_rate| RenderFormat {
                sample_rate,
                ..current
            })
        }
    }
    fn adopt_external_format(
        &self,
        requested: RenderFormat,
    ) -> Result<(), mixrelay_audio_io::HostAudioBackendError> {
        use mixrelay_audio_io::HostAudioBackendError;
        if !self.supports_format(requested)
            || (!self.native.follows_rate() && !self.native.follows_pair())
        {
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
        self.restore_failed.store(false, Ordering::Release);
        // Never call the native setter here. The caller publishes the prepared DSP plan
        // before resuming; the saved last-accepted format prevents restart undoing the DAW.
        self.failed.store(false, Ordering::Release);
        Ok(())
    }
    fn reconfigure(
        &self,
        current: RenderFormat,
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
        // Serialize and sync the new preference before suspending audio. The rename is
        // committed only after the native device has confirmed the requested format.
        let staged = self.stage_preferences(requested).map_err(|message| {
            self.preferences_error.store(true, Ordering::Release);
            HostAudioBackendError::BackendFailure { message }
        })?;
        self.pause_at_boundary()?;
        let previous_native = self
            .native_rollback_format(current)
            .map_err(|message| HostAudioBackendError::BackendFailure { message })?;
        let negotiated = self.write_native_format(requested).and_then(|()| {
            if let Some(staged) = staged {
                staged.commit().map_err(|error| {
                    self.preferences_error.store(true, Ordering::Release);
                    format!("session preferences could not be committed: {error}")
                })?;
            }
            Ok(())
        });
        if let Err(error) = negotiated {
            if let Err(rollback) = self.write_native_format(previous_native) {
                self.failed.store(true, Ordering::Release);
                return Err(HostAudioBackendError::BackendFailure {
                    message: format!(
                        "{error}; rollback failed: {rollback}; audio remains suspended"
                    ),
                });
            }
            return Err(HostAudioBackendError::BackendFailure { message: error });
        }
        if self.native.has_pair_control() {
            self.observe_device_format(Some(requested));
        } else if self.native.has_rate_control() {
            self.observe_device_rate(Some(requested.sample_rate));
        }
        if self.preferences.is_some() {
            self.preferred_format
                .store(pack_format(requested), Ordering::Release);
        }
        self.preferences_error.store(false, Ordering::Release);
        self.restore_failed.store(false, Ordering::Release);
        self.failed.store(false, Ordering::Release);
        Ok(())
    }
}
