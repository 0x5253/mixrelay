use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;

fn endpoint_capabilities() -> VirtualEndpointCapabilities {
    VirtualEndpointCapabilities {
        max_render_channels: 64,
        max_capture_channels: 64,
        supported_sample_rates: vec![44_100, 48_000, 96_000],
        supports_duplex: true,
        supports_dynamic_creation: true,
    }
}

fn stream(channels: u16, channel_layout: ChannelLayout) -> StreamSpec {
    StreamSpec {
        channels,
        channel_layout,
    }
}

fn endpoint() -> VirtualEndpointSpec {
    VirtualEndpointSpec {
        id: VirtualEndpointId::from("io.mixrelay.spat"),
        display_name: "MixRelay SPAT".into(),
        render: Some(stream(64, ChannelLayout::Discrete)),
        capture: Some(stream(2, ChannelLayout::Stereo)),
        supported_sample_rates: vec![48_000, 96_000],
        default_sample_rate: 48_000,
    }
}

fn capture_capabilities() -> ApplicationCaptureCapabilities {
    ApplicationCaptureCapabilities {
        max_channels: 8,
        supported_selectors: BTreeSet::from([
            ApplicationSelectorKind::BundleId,
            ApplicationSelectorKind::ExecutablePath,
            ApplicationSelectorKind::AllExcept,
        ]),
        supports_mute_while_captured: true,
    }
}

fn capture(selector: ApplicationSelector) -> ApplicationCaptureRequest {
    ApplicationCaptureRequest {
        selector,
        stream: stream(2, ChannelLayout::Stereo),
        source_playback: SourcePlaybackPolicy::Preserve,
        forwarding_endpoint: None,
        gain_db: 0.0,
        requires_reselection: false,
    }
}

#[test]
fn capture_omits_unset_optional_fields_and_roundtrips_reselection_without_rewriting_selector() {
    let original = capture(ApplicationSelector::ProcessId {
        process_id: 42,
        include_descendants: true,
    });
    let compact = serde_json::to_value(&original).unwrap();
    assert!(compact.get("requiresReselection").is_none());
    assert!(compact.get("forwardingEndpoint").is_none());
    let mut restored: ApplicationCaptureRequest = serde_json::from_value(compact).unwrap();
    assert!(!restored.requires_reselection);
    restored.requires_reselection = true;
    let saved = serde_json::to_value(&restored).unwrap();
    assert_eq!(saved["requiresReselection"], true);
    let reloaded: ApplicationCaptureRequest = serde_json::from_value(saved).unwrap();
    assert_eq!(reloaded.selector, original.selector);
    assert!(reloaded.requires_reselection);
    reloaded.validate_contract(64).unwrap();
}

#[test]
fn owned_forwarding_requires_an_exact_selected_source_and_endpoint() {
    let mut request = capture(ApplicationSelector::BundleId {
        bundle_id: "org.mixrelay.player".into(),
        include_descendants: false,
    });
    request.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    assert_eq!(
        request.validate_contract(8),
        Err(HostAudioContractError::ForwardingEndpointRequired)
    );

    request.forwarding_endpoint = Some(VirtualEndpointId::from("io.mixrelay.player"));
    request.validate_contract(8).unwrap();
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(encoded["forwardingEndpoint"], "io.mixrelay.player");
    let decoded: ApplicationCaptureRequest = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, request);

    request.forwarding_endpoint = Some(VirtualEndpointId::from("  "));
    assert_eq!(
        request.validate_contract(8),
        Err(HostAudioContractError::EmptyValue {
            field: "forwarding endpoint ID"
        })
    );
    request.forwarding_endpoint = Some(VirtualEndpointId::from("io.mixrelay.player"));
    request.selector = ApplicationSelector::AllExcept { bundle_ids: vec![] };
    assert_eq!(
        request.validate_contract(8),
        Err(HostAudioContractError::ForwardingRequiresExactSelection)
    );
    request.selector = ApplicationSelector::ProcessId {
        process_id: 42,
        include_descendants: false,
    };
    request.requires_reselection = true;
    assert_eq!(
        request.validate_contract(8),
        Err(HostAudioContractError::ForwardingRequiresExactSelection)
    );

    request.requires_reselection = false;
    request.source_playback = SourcePlaybackPolicy::Preserve;
    assert_eq!(
        request.validate_contract(8),
        Err(HostAudioContractError::UnexpectedForwardingEndpoint)
    );
}

#[test]
fn validates_duplex_multichannel_endpoint_contracts() {
    let endpoint = endpoint();
    endpoint
        .validate(&endpoint_capabilities())
        .expect("SPAT endpoint should be representable");

    assert_eq!(endpoint.id.as_str(), "io.mixrelay.spat");
    assert!(endpoint_capabilities().supports_dynamic_creation);

    let mut capture_only = endpoint;
    capture_only.render = None;
    capture_only
        .validate(&endpoint_capabilities())
        .expect("capture-only endpoint should be representable");
    let mut render_only = capture_only;
    render_only.render = Some(stream(64, ChannelLayout::Discrete));
    render_only.capture = None;
    render_only
        .validate(&endpoint_capabilities())
        .expect("render-only endpoint should be representable");
}

#[test]
fn endpoint_display_name_rejects_nul_and_accepts_korean() {
    let mut spec = endpoint();
    spec.display_name = "QA invalid\0name".into();
    assert_eq!(
        spec.validate_contract(64, 64),
        Err(HostAudioContractError::InteriorNul {
            field: "virtual endpoint display name"
        })
    );

    spec.display_name = "테스트 장치".into();
    spec.validate_contract(64, 64).unwrap();
}

#[test]
fn rejects_invalid_endpoint_identity_streams_and_rates() {
    let mut cases = Vec::new();
    let mut invalid = endpoint();
    invalid.id = VirtualEndpointId::from(" ");
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.display_name.clear();
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.render = None;
    invalid.capture = None;
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.render = Some(stream(0, ChannelLayout::Discrete));
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.render = Some(stream(65, ChannelLayout::Discrete));
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.capture = Some(stream(3, ChannelLayout::Stereo));
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.capture = Some(stream(2, ChannelLayout::Mono));
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.supported_sample_rates = vec![48_000, 48_000];
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.supported_sample_rates = vec![0, 48_000];
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.default_sample_rate = 44_100;
    cases.push(invalid);
    let mut invalid = endpoint();
    invalid.supported_sample_rates = vec![192_000];
    invalid.default_sample_rate = 192_000;
    cases.push(invalid);

    for invalid in cases {
        assert!(invalid.validate(&endpoint_capabilities()).is_err());
    }

    let mut capabilities = endpoint_capabilities();
    capabilities.supports_duplex = false;
    assert_eq!(
        endpoint().validate(&capabilities),
        Err(HostAudioContractError::DuplexUnsupported)
    );
}

#[test]
fn validates_every_application_selector_and_policy_capability() {
    let selectors = [
        ApplicationSelector::BundleId {
            bundle_id: "org.mozilla.firefox".into(),
            include_descendants: true,
        },
        ApplicationSelector::ExecutablePath {
            path: "/Applications/Firefox.app/Contents/MacOS/firefox".into(),
        },
        ApplicationSelector::AllExcept {
            bundle_ids: vec!["com.apple.Music".into()],
        },
    ];
    for selector in selectors {
        capture(selector)
            .validate(&capture_capabilities())
            .expect("selector should be supported");
    }

    let decoded: ApplicationSelector = serde_json::from_value(serde_json::json!({
        "match": "bundle_id",
        "bundle_id": "org.mozilla.firefox"
    }))
    .expect("selector should decode");
    assert!(matches!(
        decoded,
        ApplicationSelector::BundleId {
            include_descendants: true,
            ..
        }
    ));
}

#[test]
fn rejects_invalid_or_unsupported_application_capture_requests() {
    let invalid = [
        capture(ApplicationSelector::BundleId {
            bundle_id: String::new(),
            include_descendants: false,
        }),
        capture(ApplicationSelector::ExecutablePath {
            path: String::new(),
        }),
        capture(ApplicationSelector::AllExcept {
            bundle_ids: vec![String::new()],
        }),
    ];
    for request in invalid {
        assert!(request.validate(&capture_capabilities()).is_err());
    }

    let mut capabilities = capture_capabilities();
    capabilities
        .supported_selectors
        .remove(&ApplicationSelectorKind::AllExcept);
    let request = capture(ApplicationSelector::AllExcept { bundle_ids: vec![] });
    assert_eq!(
        request.validate(&capabilities),
        Err(HostAudioContractError::ApplicationSelectorUnsupported)
    );

    let mut request = capture(ApplicationSelector::ExecutablePath {
        path: "/verified/player".into(),
    });
    request.source_playback = SourcePlaybackPolicy::MuteWhileCaptured;
    request.forwarding_endpoint = Some(VirtualEndpointId::from("io.mixrelay.player"));
    capabilities
        .supported_selectors
        .insert(ApplicationSelectorKind::ExecutablePath);
    capabilities.supports_mute_while_captured = false;
    assert_eq!(
        request.validate(&capabilities),
        Err(HostAudioContractError::MuteWhileCapturedUnsupported)
    );
}

#[derive(Default)]
struct FakeEndpointBackend {
    desired: Mutex<Vec<VirtualEndpointSpec>>,
}

impl VirtualEndpointBackend for FakeEndpointBackend {
    fn capabilities(&self) -> &VirtualEndpointCapabilities {
        static CAPABILITIES: std::sync::LazyLock<VirtualEndpointCapabilities> =
            std::sync::LazyLock::new(endpoint_capabilities);
        &CAPABILITIES
    }

    fn prepare_graph_attachments(
        &self,
        desired: &[VirtualEndpointSpec],
    ) -> Result<(), HostAudioBackendError> {
        *self.desired.lock().expect("fake lock should be healthy") = desired.to_vec();
        Ok(())
    }
}

#[derive(Default)]
struct FakeCaptureBackend {
    dropped_sessions: Arc<AtomicUsize>,
}

struct FakeCaptureSession {
    resolved: ResolvedApplicationCapture,
    dropped_sessions: Arc<AtomicUsize>,
}

impl ApplicationCaptureSession for FakeCaptureSession {
    fn resolved(&self) -> &ResolvedApplicationCapture {
        &self.resolved
    }
}

impl Drop for FakeCaptureSession {
    fn drop(&mut self) {
        self.dropped_sessions.fetch_add(1, Ordering::Relaxed);
    }
}

impl ApplicationCaptureBackend for FakeCaptureBackend {
    type Session = FakeCaptureSession;

    fn capabilities(&self) -> &ApplicationCaptureCapabilities {
        static CAPABILITIES: std::sync::LazyLock<ApplicationCaptureCapabilities> =
            std::sync::LazyLock::new(capture_capabilities);
        &CAPABILITIES
    }

    fn open(
        &self,
        request: &ApplicationCaptureRequest,
    ) -> Result<Self::Session, HostAudioBackendError> {
        request.validate(self.capabilities()).map_err(|error| {
            HostAudioBackendError::CapabilityUnavailable {
                capability: error.to_string(),
            }
        })?;
        Ok(FakeCaptureSession {
            resolved: ResolvedApplicationCapture {
                source_id: "capture.firefox".into(),
                stream: request.stream,
            },
            dropped_sessions: Arc::clone(&self.dropped_sessions),
        })
    }
}

#[test]
fn composes_endpoint_and_application_backends_without_inheritance() {
    let services = HostAudioServices::new(
        FakeEndpointBackend::default(),
        FakeCaptureBackend::default(),
    );
    services
        .virtual_endpoints()
        .prepare_graph_attachments(&[endpoint()])
        .expect("endpoint inventory should reconcile");
    assert_eq!(
        services
            .virtual_endpoints()
            .desired
            .lock()
            .expect("fake lock should be healthy")
            .len(),
        1
    );
    assert_eq!(
        services
            .virtual_endpoints()
            .capabilities()
            .max_render_channels,
        64
    );

    {
        let session = services
            .application_capture()
            .open(&capture(ApplicationSelector::BundleId {
                bundle_id: "org.mozilla.firefox".into(),
                include_descendants: true,
            }))
            .expect("capture should open");
        assert_eq!(session.resolved().source_id, "capture.firefox");
        assert_eq!(
            services
                .application_capture()
                .dropped_sessions
                .load(Ordering::Relaxed),
            0
        );
    }
    assert_eq!(
        services
            .application_capture()
            .dropped_sessions
            .load(Ordering::Relaxed),
        1
    );
}

#[test]
fn exposes_stable_backend_error_messages() {
    let errors = [
        HostAudioBackendError::PermissionDenied {
            capability: "application_capture".into(),
        },
        HostAudioBackendError::CapabilityUnavailable {
            capability: "virtual_endpoint".into(),
        },
        HostAudioBackendError::BackendFailure {
            message: "driver stopped".into(),
        },
    ];
    assert_eq!(
        errors[0].to_string(),
        "host audio permission denied: application_capture"
    );
    assert!(errors.iter().all(|error| !error.to_string().is_empty()));
}
