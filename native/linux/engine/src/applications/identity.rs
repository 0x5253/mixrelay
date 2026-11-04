//! Canonical `PipeWire` capture intent. Registry serials and process pins are resolved
//! by the native owner, never used as persistent application names or graph IDs.
use mixrelay_audio_graph::NodeConfig;
use mixrelay_audio_graph::composition::SharedSource;
use mixrelay_audio_io::{
    ApplicationCaptureRequest, ApplicationSelector, ChannelLayout, HostAudioBackendError,
};
use sha2::{Digest, Sha256};

/// Resolves capture intent into a graph-independent native request with zero gain.
/// Returns a stable source identity; channel width remains available for catalog
/// merging. Rejects invalid, unsupported or unselected requests before native I/O.
pub(crate) fn resolve(
    request: &ApplicationCaptureRequest,
) -> Result<SharedSource, HostAudioBackendError> {
    request
        .validate_contract(crate::runtime::MAX_ENDPOINT_CHANNELS)
        .map_err(|error| unsupported(error.to_string()))?;
    if request.requires_reselection {
        return Err(unsupported(
            "application capture requires explicit reselection",
        ));
    }
    if matches!(
        request.selector,
        ApplicationSelector::BundleId {
            include_descendants: true,
            ..
        }
    ) {
        return Err(unsupported(
            "PipeWire application IDs require exact selection without descendants",
        ));
    }
    let mut canonical = request.clone();
    if let ApplicationSelector::AllExcept { bundle_ids } = &mut canonical.selector {
        bundle_ids.sort_unstable();
        bundle_ids.dedup();
    }
    // No display-name/basename aliases: an executable family, exact application ID,
    // PID family and all-except set have different capture scope even if today's
    // inventory overlaps. Gain and width do not create a second native owner.
    let selector = serde_json::to_vec(&canonical.selector).map_err(|error| {
        HostAudioBackendError::BackendFailure {
            message: error.to_string(),
        }
    })?;
    canonical.gain_db = 0.0;
    canonical.stream.channel_layout = ChannelLayout::Discrete;
    Ok(SharedSource {
        id: format!("pipewire:{:x}", Sha256::digest(selector)),
        config: NodeConfig::ApplicationCapture(canonical),
    })
}

fn unsupported(capability: impl Into<String>) -> HostAudioBackendError {
    HostAudioBackendError::CapabilityUnavailable {
        capability: capability.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mixrelay_audio_io::{SourcePlaybackPolicy, StreamSpec};

    fn request(selector: ApplicationSelector) -> ApplicationCaptureRequest {
        ApplicationCaptureRequest {
            selector,
            stream: StreamSpec {
                channels: 2,
                channel_layout: ChannelLayout::Stereo,
            },
            source_playback: SourcePlaybackPolicy::Preserve,
            forwarding_endpoint: None,
            gain_db: 0.0,
            requires_reselection: false,
        }
    }

    #[test]
    fn stable_intent_is_independent_of_graph_gain_and_channel_projection() {
        let stereo = request(ApplicationSelector::ExecutablePath {
            path: "/usr/bin/player".into(),
        });
        let mut mono = stereo.clone();
        mono.gain_db = -12.0;
        mono.stream = StreamSpec {
            channels: 1,
            channel_layout: ChannelLayout::Mono,
        };
        let a = resolve(&stereo).unwrap();
        let b = resolve(&mono).unwrap();
        assert_eq!(a.id, b.id);
        let NodeConfig::ApplicationCapture(canonical) = b.config else {
            panic!("capture")
        };
        assert_eq!(canonical.gain_db.to_bits(), 0.0_f32.to_bits());
        assert_eq!(canonical.stream.channels, 1);
        assert_eq!(mono.gain_db.to_bits(), (-12.0_f32).to_bits());
    }

    #[test]
    fn exclusion_sets_are_order_independent_but_capture_scopes_do_not_alias() {
        let a = request(ApplicationSelector::AllExcept {
            bundle_ids: vec!["b".into(), "a".into(), "b".into()],
        });
        let b = request(ApplicationSelector::AllExcept {
            bundle_ids: vec!["a".into(), "b".into()],
        });
        assert_eq!(resolve(&a).unwrap(), resolve(&b).unwrap());
        let bundle = request(ApplicationSelector::BundleId {
            bundle_id: "/usr/bin/player".into(),
            include_descendants: false,
        });
        let executable = request(ApplicationSelector::ExecutablePath {
            path: "/usr/bin/player".into(),
        });
        assert_ne!(
            resolve(&bundle).unwrap().id,
            resolve(&executable).unwrap().id
        );
    }

    #[test]
    fn unsupported_or_unselected_capture_is_rejected_before_native_preparation() {
        let mut capture = request(ApplicationSelector::ProcessId {
            process_id: 42,
            include_descendants: true,
        });
        capture.requires_reselection = true;
        assert!(resolve(&capture).is_err());
        capture.requires_reselection = false;
        capture.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
        assert!(resolve(&capture).is_err()); // Exact forwarding endpoint is required.
        capture.forwarding_endpoint = Some("owned-forward".into());
        assert_eq!(
            resolve(&capture).unwrap().id,
            resolve(&request(capture.selector.clone())).unwrap().id
        );
        capture.source_playback = SourcePlaybackPolicy::Preserve;
        capture.forwarding_endpoint = None;
        capture.selector = ApplicationSelector::BundleId {
            bundle_id: "player".into(),
            include_descendants: true,
        };
        assert!(resolve(&capture).is_err());
        capture.selector = ApplicationSelector::AllExcept {
            bundle_ids: Vec::new(),
        };
        capture.stream.channels = 65;
        assert!(resolve(&capture).is_err());
    }
}
