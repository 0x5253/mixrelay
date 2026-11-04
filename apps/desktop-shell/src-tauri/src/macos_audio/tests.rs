//! Capture requests rejected before any native tap opens.

use super::*;
use mixrelay_audio_io::{ApplicationCaptureBackend, HostAudioBackendError};

#[test]
fn source_mute_without_owned_forwarding_is_rejected_before_native_capture() {
    let request = serde_json::from_value(serde_json::json!({
        "selector":{"match":"bundle_id","bundle_id":"io.mixrelay.unowned-fixture","include_descendants":false},
        "stream":{"channels":2,"channelLayout":"stereo"},
        "sourcePlayback":"mute_while_captured"
    }))
    .unwrap();
    let backend = MacApplicationCaptureBackend::default();
    assert!(!backend.capabilities().supports_mute_while_captured);
    assert!(matches!(
        backend.open(&request),
        Err(HostAudioBackendError::CapabilityUnavailable { .. })
    ));
}

#[test]
fn restored_pid_manifest_is_rejected_without_attempting_a_native_tap() {
    let manifest = serde_json::from_value(serde_json::json!({
        "protocolVersion":1, "revision":7, "sources":[{
            "nodeId":"saved-pid", "request":{
                "selector":{"match":"process_id","process_id":4_294_967_295_u32,"include_descendants":true},
                "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve","requiresReselection":true
            }
        }]
    })).unwrap();
    let state = ApplicationCaptureState::default();
    assert!(state.prepare(&manifest).is_err());
    assert!(!state.has_pending().unwrap());
}
