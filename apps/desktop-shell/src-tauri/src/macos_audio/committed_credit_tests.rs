//! Converted shared PCM owns graph credit independently of the tap's raw backlog.
use super::*;
use mixrelay_macos_transport::{
    application_source_transport::mr_application_source_transport_attach,
    source_control::{SourceAttachment, SourceRequest},
    source_registry::{SourceRegistry, mr_engine_source_id_token},
};
use std::{ffi::c_char, os::fd::IntoRawFd};

fn source() -> (SourceRegistry, ApplicationSourceTransport, u64) {
    let registry = SourceRegistry::new(
        Box::new(mixrelay_time::ManualClock::default()),
        mixrelay_realtime::Timebase::from_ratio(
            std::num::NonZeroU32::new(1).unwrap(),
            std::num::NonZeroU32::new(1).unwrap(),
        ),
    );
    let mut request = SourceRequest {
        capacity_frames: 4096,
        channels: 2,
        transport_version: mixrelay_macos_transport::application_clock::VERSION,
        ..Default::default()
    };
    for (output, byte) in request.source_id.iter_mut().zip(b"committed-credit") {
        *output = c_char::from_ne_bytes([*byte]);
    }
    let grant = registry.attach(&request).unwrap();
    let mut attachment = SourceAttachment {
        descriptor: grant.descriptor.into_raw_fd(),
        mapped_size: grant.mapped_size,
        producer_lease: grant.producer_lease,
        source_epoch: grant.source_epoch,
    };
    // SAFETY: this fixture transfers its unique producer attachment to the production owner.
    let pointer = unsafe {
        mr_application_source_transport_attach(&raw mut attachment, 4096, 2, std::ptr::null_mut())
    };
    let mut transport = ApplicationSourceTransport {
        native: NonNull::new(pointer.cast()).unwrap(),
    };
    // SAFETY: the source ID is a retained NUL-terminated string.
    let token = unsafe { mr_engine_source_id_token(c"committed-credit".as_ptr()) };
    assert_eq!(registry.read_audio(token, &mut [0.0; 256], 128, 48_000), 0);
    let format = transport.clock().unwrap().format_word;
    assert!(transport.reset(format, 24_000));
    (registry, transport, token)
}

#[test]
fn raw_capture_backlog_cannot_authorize_unwritten_source_pcm() {
    let (registry, mut transport, token) = source();
    let mut health = TapStats {
        callback_count: 480,
        captured_frames: 245_760,
        dropped_frames: 512,
        available_frames: 245_760,
        channels: 2,
        sample_rate: 24_000.0,
        running: true,
        format_ready: true,
        format_generation: 1,
        clock_discontinuities: 0,
        sample_time: 245_760.0,
        host_time: 1,
        clock_valid: true,
    };
    transport.observe_capture(&health);
    assert_eq!(
        registry.clock(token).unwrap().native_frames,
        0,
        "raw captured backlog was published before shared PCM conversion"
    );
    assert!(transport.write(&[0.25; 256], &[100; 128], 128));
    assert_eq!(transport.clock().unwrap().produced_frames, 128);
    assert_eq!(registry.clock(token).unwrap().native_frames, 64);
    transport.observe_capture(&health);
    assert_eq!(registry.clock(token).unwrap().native_frames, 64);
    assert!(transport.write(&[0.0; 128], &[200; 64], 64));
    assert_eq!(registry.clock(token).unwrap().native_frames, 96);
    health.running = false;
    transport.observe_capture(&health);
    let clock = registry.clock(token).unwrap();
    assert_eq!(clock.native_frames, 96);
    assert_eq!(clock.running, 0);
}
