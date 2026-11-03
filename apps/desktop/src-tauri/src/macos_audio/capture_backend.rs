//! Application selector capabilities and forwarding validation before tap creation.

use super::CaptureProbeSession;
use mixrelay_audio_graph::{ApplicationCaptureRequest, SourcePlaybackPolicy};
use mixrelay_audio_io::{
    ApplicationCaptureBackend, ApplicationCaptureCapabilities, ApplicationSelectorKind,
    HostAudioBackendError,
};
use mixrelay_macos_transport::endpoint_identity::{endpoint_uid, parse_endpoint_id};
use std::collections::BTreeSet;

pub(super) struct MacApplicationCaptureBackend {
    capabilities: ApplicationCaptureCapabilities,
}

impl Default for MacApplicationCaptureBackend {
    fn default() -> Self {
        Self {
            capabilities: ApplicationCaptureCapabilities {
                max_channels: 2,
                supported_selectors: BTreeSet::from([
                    ApplicationSelectorKind::BundleId,
                    ApplicationSelectorKind::AllExcept,
                ]),
                supports_mute_while_captured: false,
            },
        }
    }
}

impl ApplicationCaptureBackend for MacApplicationCaptureBackend {
    type Session = CaptureProbeSession;

    fn capabilities(&self) -> &ApplicationCaptureCapabilities {
        &self.capabilities
    }

    fn open(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<Self::Session, HostAudioBackendError> {
        request.validate(&self.capabilities).map_err(|error| {
            HostAudioBackendError::CapabilityUnavailable {
                capability: error.to_string(),
            }
        })?;
        let target_device_uid = match request.source_playback {
            SourcePlaybackPolicy::Preserve => None,
            SourcePlaybackPolicy::MuteWhileCaptured => {
                let id = request
                    .forwarding_endpoint
                    .as_ref()
                    .and_then(|endpoint| parse_endpoint_id(endpoint.as_str()))
                    .ok_or_else(|| HostAudioBackendError::CapabilityUnavailable {
                        capability: "forwarding requires an exact MixRelay endpoint identity"
                            .to_owned(),
                    })?;
                Some(endpoint_uid(id))
            }
        };
        CaptureProbeSession::prepare_selector(&request.selector, target_device_uid.as_deref())
            .and_then(|session| {
                session.start_native()?;
                Ok(session)
            })
            .map_err(|error| HostAudioBackendError::BackendFailure {
                message: error.to_string(),
            })
    }
}
