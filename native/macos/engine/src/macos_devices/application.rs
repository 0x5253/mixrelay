use mixrelay_audio_graph::NodeConfig;
use mixrelay_audio_graph::composition::SharedSource;
use mixrelay_audio_io::{
    ApplicationCaptureRequest, ApplicationSelector, ChannelLayout, HostAudioBackendError,
    SourcePlaybackPolicy, StreamSpec,
};

/// The native process tap mixes an exact bundle into stereo; it does not walk
/// process descendants. Local channel projection and gain belong to graph DSP.
pub(super) fn resolve(
    request: &ApplicationCaptureRequest,
) -> Result<SharedSource, HostAudioBackendError> {
    request
        .validate_contract(2)
        .map_err(|error| unsupported(&error.to_string()))?;
    let source_id = mixrelay_macos_transport::capture_selection::source_id(&request.selector)
        .ok_or_else(|| {
            unsupported("macOS capture requires an exact bundle or whole-system selection")
        })?;
    if request.requires_reselection {
        return Err(unsupported(
            "application capture requires explicit reselection",
        ));
    }
    if request.source_playback == SourcePlaybackPolicy::MuteWhileCaptured
        && request
            .forwarding_endpoint
            .as_ref()
            .and_then(|id| {
                mixrelay_macos_transport::endpoint_identity::parse_endpoint_id(id.as_str())
            })
            .is_none()
    {
        return Err(unsupported(
            "forwarding requires an exact MixRelay endpoint identity",
        ));
    }
    let mut canonical = request.clone();
    if let ApplicationSelector::AllExcept { bundle_ids } = &mut canonical.selector {
        bundle_ids.sort_unstable();
        bundle_ids.dedup();
    }
    canonical.gain_db = 0.0;
    canonical.stream = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    Ok(SharedSource {
        id: source_id,
        config: NodeConfig::ApplicationCapture(canonical),
    })
}

pub(super) fn validate_playback(
    playback: &mixrelay_engine::PlaybackBindings<'_>,
) -> Result<(), HostAudioBackendError> {
    let mixrelay_engine::PlaybackBindings::Forward { intents, controls } = playback else {
        return Ok(());
    };
    let mut endpoints = std::collections::BTreeMap::new();
    for intent in *intents {
        if !controls
            .snapshot(&intent.mask_node_id)
            .is_some_and(|(control, _)| control.enabled && !control.muted)
        {
            continue;
        }
        if mixrelay_macos_transport::endpoint_identity::parse_endpoint_id(
            intent.forwarding_endpoint.as_str(),
        )
        .is_none()
        {
            return Err(unsupported(
                "forwarding requires an exact MixRelay endpoint identity",
            ));
        }
        if endpoints
            .insert(&intent.source_id, &intent.forwarding_endpoint)
            .is_some_and(|previous| previous != &intent.forwarding_endpoint)
        {
            return Err(unsupported(
                "one application source cannot suppress different forwarding endpoints",
            ));
        }
    }
    Ok(())
}

fn unsupported(capability: &str) -> HostAudioBackendError {
    HostAudioBackendError::CapabilityUnavailable {
        capability: capability.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ApplicationCaptureRequest {
        ApplicationCaptureRequest {
            selector: ApplicationSelector::BundleId {
                bundle_id: "org.mixrelay.capture-fixture".into(),
                include_descendants: false,
            },
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
    fn system_capture_is_one_canonical_stereo_source_with_set_exclusions() {
        let mut first = request();
        first.selector = ApplicationSelector::AllExcept {
            bundle_ids: vec!["b".into(), "a".into(), "a".into()],
        };
        let mut second = first.clone();
        second.selector = ApplicationSelector::AllExcept {
            bundle_ids: vec!["a".into(), "b".into()],
        };
        second.gain_db = -12.0;
        second.stream.channels = 1;
        second.stream.channel_layout = ChannelLayout::Mono;
        assert_eq!(resolve(&first).unwrap(), resolve(&second).unwrap());
        assert_ne!(resolve(&first).unwrap().id, resolve(&request()).unwrap().id);
        second.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
        assert!(resolve(&second).is_err());
    }

    #[test]
    fn local_mono_projection_and_gain_share_one_stereo_native_owner() {
        let stereo = request();
        let mut mono = stereo.clone();
        mono.stream.channels = 1;
        mono.stream.channel_layout = ChannelLayout::Mono;
        mono.gain_db = -12.0;
        assert_eq!(resolve(&stereo).unwrap(), resolve(&mono).unwrap());
        assert_eq!(mono.stream.channels, 1);
        assert_eq!(mono.gain_db.to_bits(), (-12.0_f32).to_bits());
        let NodeConfig::ApplicationCapture(canonical) = resolve(&mono).unwrap().config else {
            panic!("application source")
        };
        assert_eq!(canonical.stream.channels, 2);
        assert_eq!(canonical.gain_db.to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn source_mute_requires_an_exact_owned_endpoint_without_changing_capture_identity() {
        let preserve = resolve(&request()).unwrap();
        let mut muted = request();
        muted.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
        let unowned = muted.clone();
        assert!(matches!(
            resolve(&muted),
            Err(HostAudioBackendError::CapabilityUnavailable { .. })
        ));
        assert_eq!(unowned, muted);
        muted.forwarding_endpoint = Some(mixrelay_audio_io::VirtualEndpointId::from(
            "17e59d73-42c1-4f11-a8b4-6c2d913357e0",
        ));
        let owned = muted.clone();
        let resolved = resolve(&muted).unwrap();
        assert_eq!(resolved.id, preserve.id);
        let NodeConfig::ApplicationCapture(resolved) = resolved.config else {
            panic!("application capture")
        };
        assert_eq!(
            resolved.source_playback,
            SourcePlaybackPolicy::MuteWhileCaptured
        );
        assert_eq!(resolved.forwarding_endpoint, muted.forwarding_endpoint);
        assert_eq!(owned, muted);
        assert_eq!(preserve, resolve(&request()).unwrap());
        let mut other = request();
        other.selector = ApplicationSelector::BundleId {
            bundle_id: "org.mixrelay.other-fixture".into(),
            include_descendants: false,
        };
        assert_ne!(preserve.id, resolve(&other).unwrap().id);
    }

    #[test]
    fn unsupported_selectors_and_unresolved_intent_are_never_relabelled_as_bundle_capture() {
        for selector in [
            ApplicationSelector::BundleId {
                bundle_id: "org.mixrelay.capture-fixture".into(),
                include_descendants: true,
            },
            ApplicationSelector::ExecutablePath {
                path: "/Applications/Fixture.app".into(),
            },
            ApplicationSelector::ProcessId {
                process_id: 42,
                include_descendants: false,
            },
        ] {
            let mut unsupported = request();
            unsupported.selector = selector;
            assert!(resolve(&unsupported).is_err());
        }
        let mut unresolved = request();
        unresolved.requires_reselection = true;
        assert!(resolve(&unresolved).is_err());
        let mut invalid = request();
        invalid.stream.channels = 3;
        assert!(resolve(&invalid).is_err());
        invalid = request();
        invalid.gain_db = f32::NAN;
        assert!(resolve(&invalid).is_err());
    }
}
