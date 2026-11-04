//! Registry fixtures: real mappings and producer facades, without hardware.
mod content_latency;
mod reads;
mod registration;
mod time_reads;
use super::identity::token;
use super::registration::MAX_RETAINED;
use super::slots::MAX_SOURCES;
use super::*;
use crate::{
    application_clock::{VERSION, mr_application_clock_request_rate},
    application_source_transport::{
        SourceTransport, mr_application_source_transport_attach,
        mr_application_source_transport_publish_native, mr_application_source_transport_reset,
        mr_application_source_transport_write,
    },
    shared_audio_buffer::{
        mr_shared_audio_buffer_request_epoch, mr_shared_audio_buffer_requires_replacement,
        mr_shared_audio_buffer_stats, mr_shared_audio_buffer_try_begin_epoch,
        mr_shared_audio_buffer_write,
    },
    source_control::{SourceAttachment, SourceRequest},
};
use mixrelay_time::Instant;
use std::{ffi::c_char, os::fd::IntoRawFd, ptr};

fn producer(registry: &SourceRegistry, request: &SourceRequest) -> SourceTransport {
    let attached = registry.attach(request).unwrap();
    let mut attachment = SourceAttachment {
        descriptor: attached.descriptor.into_raw_fd(),
        mapped_size: attached.mapped_size,
        producer_lease: attached.producer_lease,
        source_epoch: attached.source_epoch,
    };
    let owner = unsafe {
        mr_application_source_transport_attach(
            &raw mut attachment,
            request.capacity_frames,
            request.channels,
            ptr::null_mut(),
        )
    };
    assert!(!owner.is_null());
    // Move the unique facade owner out of its allocation; its real destructor
    // still closes the lease and mapping when this test-owned value drops.
    *unsafe { Box::from_raw(owner) }
}

fn collect(registry: &SourceRegistry) {
    registry.collect(&mut registry.control());
}

fn request(id: &str) -> SourceRequest {
    let mut request = SourceRequest {
        capacity_frames: 1024,
        channels: 2,
        transport_version: VERSION,
        ..SourceRequest::default()
    };
    for (target, byte) in request.source_id.iter_mut().zip(id.bytes()) {
        *target = c_char::from_ne_bytes([byte]);
    }
    request
}

fn timebase() -> mixrelay_realtime::Timebase {
    mixrelay_realtime::Timebase::from_ratio(
        std::num::NonZeroU32::new(1).unwrap(),
        std::num::NonZeroU32::new(1).unwrap(),
    )
}

fn registry() -> SourceRegistry {
    SourceRegistry::new(Box::new(FixedSchedulingClock), timebase())
}

struct FixedSchedulingClock;

impl mixrelay_time::MonotonicClock for FixedSchedulingClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(std::time::Duration::ZERO)
    }
}
