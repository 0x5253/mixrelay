use super::*;
const MAX_FRAME: u64 = (1 << 48) - 1;
use crate::endpoint_transport::{self as endpoint, EndpointClockSnapshot};
use std::{
    alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error},
    ptr::NonNull,
};

struct Storage {
    allocation: NonNull<u8>,
    layout: Layout,
    transport: *mut EndpointTransport,
}
impl Storage {
    fn new(render_channels: u32, capture_channels: u32) -> Self {
        let config = EndpointConfig {
            endpoint_id: [1; 16],
            capacity_frames: 16,
            render_channels,
            capture_channels,
        };
        let bytes = unsafe { endpoint::mr_endpoint_transport_required_bytes(&raw const config) };
        let layout = Layout::from_size_align(bytes, 16).unwrap();
        let allocation = NonNull::new(unsafe { alloc_zeroed(layout) })
            .unwrap_or_else(|| handle_alloc_error(layout));
        let transport = unsafe {
            endpoint::mr_endpoint_transport_initialize(
                allocation.as_ptr().cast(),
                bytes,
                &raw const config,
            )
        };
        assert!(!transport.is_null());
        Self {
            allocation,
            layout,
            transport,
        }
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        unsafe { dealloc(self.allocation.as_ptr(), self.layout) };
    }
}
struct Fixture {
    context: HalIoContext,
    endpoint: Storage,
    render: *mut SharedAudioBuffer,
    capture: *mut SharedAudioBuffer,
}
impl Fixture {
    fn new() -> Self {
        Self::with_channels(2, 2)
    }
    fn with_channels(render_channels: u32, capture_channels: u32) -> Self {
        let endpoint = Storage::new(render_channels, capture_channels);
        let render = unsafe { mr_endpoint_transport_render_buffer(endpoint.transport) };
        let capture = unsafe { mr_endpoint_transport_capture_buffer(endpoint.transport) };
        let mut context = HalIoContext::new(render_channels, capture_channels).unwrap();
        assert!(unsafe { mr_hal_io_bind(&raw mut context, endpoint.transport.cast()) });
        unsafe {
            mr_hal_io_clock_state(&raw mut context, 48_000, false);
        }
        Self {
            context,
            endpoint,
            render,
            capture,
        }
    }
    fn clock(&self) -> EndpointClockSnapshot {
        let mut snapshot = EndpointClockSnapshot::default();
        assert!(unsafe {
            endpoint::mr_endpoint_transport_clock_snapshot(
                self.endpoint.transport,
                &raw mut snapshot,
            )
        });
        snapshot
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        unsafe { mr_hal_io_unbind(&raw mut self.context) };
    }
}

#[test]
fn context_abi_and_integral_sample_domain_are_exact() {
    assert_eq!(std::mem::size_of::<HalIoContext>(), 56);
    assert_eq!(std::mem::offset_of!(HalIoContext, capture_channels), 52);
    for (render, capture) in [(0, 0), (65, 2), (2, 65)] {
        assert!(HalIoContext::new(render, capture).is_none());
    }
    for (render, capture) in [(2, 0), (0, 2), (64, 64)] {
        assert!(HalIoContext::new(render, capture).is_some());
    }
    let mut frame = 99;
    for time in [-1.0, f64::NAN, f64::INFINITY, 0.5, MAX_EXACT_FRAME + 1.0] {
        assert!(!unsafe { mr_hal_io_sample_frame(time, &raw mut frame) });
    }
    for (time, expected) in [
        (0.0, 0),
        (-0.0, 0),
        (1.0, 1),
        (12345.0, 12345),
        (MAX_EXACT_FRAME, MAX_FRAME),
    ] {
        assert!(unsafe { mr_hal_io_sample_frame(time, &raw mut frame) });
        assert_eq!(frame, expected);
    }
    assert!(!unsafe { mr_hal_io_sample_frame(0.0, ptr::null_mut()) });
}

#[test]
fn exhausted_render_generation_refuses_start_and_preserves_pending_mapping() {
    let mut fixture = Fixture::new();
    unsafe {
        let initial = buffer::mr_shared_audio_buffer_stats(fixture.render).epoch;
        for _ in initial..u64::from(u16::MAX) {
            assert_ne!(
                buffer::mr_shared_audio_buffer_try_begin_epoch(fixture.render),
                0
            );
        }
        assert!(!mr_hal_io_begin_stream(&raw mut fixture.context));
        assert!(mr_hal_io_requires_replacement(&raw const fixture.context));
        assert!(buffer::mr_shared_audio_buffer_epoch_pending(fixture.render));
        assert!(!mr_hal_io_begin_stream(&raw mut fixture.context));
        assert_eq!(
            buffer::mr_shared_audio_buffer_try_begin_epoch(fixture.render),
            0
        );
        assert!(!buffer::mr_shared_audio_buffer_write(
            fixture.render,
            initial,
            0,
            [0.5_f32; 2].as_ptr(),
            1
        ));
    }
    let mut snapshot = EndpointClockSnapshot::default();
    assert!(!unsafe {
        endpoint::mr_endpoint_transport_clock_snapshot(
            fixture.endpoint.transport,
            &raw mut snapshot,
        )
    });
}

#[test]
fn invalid_bind_preserves_attachment_and_unbind_stops_public_clock() {
    let mut fixture = Fixture::new();
    let before = fixture.clock();
    let mismatched = Storage::new(1, 1);
    assert!(!unsafe { mr_hal_io_bind(&raw mut fixture.context, mismatched.transport.cast()) });
    assert_eq!(fixture.clock(), before);
    unsafe { mr_hal_io_unbind(&raw mut fixture.context) };
    assert_eq!(fixture.clock().running, 0);
    assert!(!unsafe { mr_hal_io_begin_stream(&raw mut fixture.context) });
}

#[test]
fn unattached_start_preserves_lifecycle_for_later_binding() {
    let mut previous = HalIoContext::new(2, 2).unwrap();
    unsafe {
        assert!(!mr_hal_io_begin_stream(&raw mut previous));
        mr_hal_io_clock_state(&raw mut previous, 96000, true);
    }
    assert_eq!(previous.render_activity.load(Ordering::Acquire), 2);
    let mut replacement = HalIoContext::new(2, 2).unwrap();
    replacement.inherit_lifecycle(&previous);
    assert!(replacement.transport.load(Ordering::Acquire).is_null());
    let transport = Storage::new(2, 2);
    unsafe {
        assert!(mr_hal_io_bind(
            &raw mut replacement,
            transport.transport.cast()
        ));
        let render = mr_endpoint_transport_render_buffer(transport.transport);
        assert_eq!(buffer::mr_shared_audio_buffer_producer_activity(render), 2);
        let mut clock = EndpointClockSnapshot::default();
        assert!(!endpoint::mr_endpoint_transport_clock_snapshot(
            transport.transport,
            &raw mut clock
        ));
        assert!(mr_hal_io_begin_stream(&raw mut replacement));
        mr_hal_io_clock_state(&raw mut replacement, 96_000, true);
        assert!(endpoint::mr_endpoint_transport_clock_snapshot(
            transport.transport,
            &raw mut clock
        ));
        assert_eq!(clock.sample_rate, 96000);
        assert_eq!(clock.running, 1);
        mr_hal_io_unbind(&raw mut replacement);
    }
}

#[test]
fn remote_snapshot_thread_cannot_observe_old_clock_with_reset_pcm_epochs() {
    let mut fixture = Fixture::new();
    unsafe { mr_hal_io_clock_state(&raw mut fixture.context, 48_000, true) };
    let previous = fixture.clock();
    let transport = unsafe { &*fixture.endpoint.transport };
    let capture = unsafe { &*fixture.capture };
    assert!(unsafe { mr_hal_io_begin_stream(&raw mut fixture.context) });
    let (ready, wait) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut snapshot = previous;
            // Use the actual shared clock and ring on a remote observer thread.
            for _ in 0..1000 {
                assert!(!unsafe {
                    endpoint::mr_endpoint_transport_clock_snapshot(transport, &raw mut snapshot)
                });
                assert_eq!(snapshot, previous);
                assert!(!unsafe { buffer::mr_shared_audio_buffer_epoch_pending(capture) });
            }
            ready.send(()).unwrap();
        });
        wait.recv().unwrap();
        unsafe { mr_hal_io_clock_state(&raw mut fixture.context, 96_000, true) };
    });
    let committed = fixture.clock();
    assert_eq!(committed.sample_rate, 96_000);
    assert!(committed.epoch > previous.epoch);
}
