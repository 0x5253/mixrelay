use super::native::Native;
use super::*;
mod format_snapshot;
mod retirement;
mod service_restart;
mod start_stop;
mod watchdog;
use mixrelay_macos_transport::{
    audio_buffer::{AudioBuffer, AudioBufferList},
    capture_ring::{mr_audio_ring_create, mr_audio_ring_read_interleaved},
};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
};

fn format() -> TapFormat {
    TapFormat {
        sample_rate: 48_000.0,
        format_id: u32::from_be_bytes(*b"lpcm"),
        flags: 9,
        bytes_per_packet: 8,
        frames_per_packet: 1,
        bytes_per_frame: 8,
        channels: 2,
        bits: 32,
    }
}

fn fixture() -> TapSession {
    let session = TapSession::default();
    session.resources.set(Resources {
        tap: 1,
        aggregate: 2,
        proc: std::ptr::dangling_mut(),
        listeners: ListenerReceipts {
            format: true,
            ..ListenerReceipts::default()
        },
        resume: false,
        start_attempted: true,
        rebuild: RebuildIntent::Unchanged,
    });
    session.format.set(format()).expect("unpublished format");
    let ring = mr_audio_ring_create(256, 2);
    assert!(!ring.is_null());
    session.ring.set(RingOwner(ring)).expect("unpublished ring");
    session
        .sample_rate
        .store(48_000_f64.to_bits(), Ordering::Release);
    session.generation.store(1, Ordering::Release);
    session.running.store(true, Ordering::Release);
    session.delivery.store(true, Ordering::Release);
    session
}

fn fallback_fixture() -> TapSession {
    let session = fixture();
    let _previous = session.selection.replace(Some(native::Selection {
        bundle: c"io.mixrelay.target".to_owned(),
        exclusions: None,
        fallback: true,
        selected: vec![native::ProcessIdentity { object: 7, pid: 71 }],
        target_device_uid: None,
        target_device: None,
        mute: false,
    }));
    let mut resources = session.resources.get();
    resources.listeners.inventory = true;
    session.resources.set(resources);
    session
}

fn muted_fixture() -> TapSession {
    let session = fallback_fixture();
    {
        let mut selection = session.selection.borrow_mut();
        let selected = selection.as_mut().expect("selected fixture");
        selected.target_device_uid =
            Some(c"io.mixrelay.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0".to_owned());
        selected.target_device = Some(native::DeviceLease {
            object: 7,
            stream: 70,
        });
        selected.mute = true;
    }
    session
}

#[test]
fn whole_system_capture_rebuilds_when_the_excluded_app_exits() {
    let session = fallback_fixture();
    session.selection.borrow_mut().as_mut().unwrap().exclusions =
        Some(vec![c"io.mixrelay.target".into()]);
    let native = Fake::default();
    assert_eq!(session.refresh(&native), Ok(()));
    assert!(session.allowed());
    assert_eq!(native.rebuild_calls.get(), 1);
    assert!(
        session
            .selection
            .borrow()
            .as_ref()
            .unwrap()
            .selected
            .is_empty()
    );
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 8, pid: 72 }]);
    assert_eq!(session.refresh(&native), Ok(()));
    assert!(session.allowed());
    assert_eq!(native.rebuild_calls.get(), 2);
    assert_eq!(
        session.selection.borrow().as_ref().unwrap().selected[0].pid,
        72
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    ServiceListener,
    InventoryListener,
    SystemInstall,
    None,
    Stop,
    Unregister,
    Listener,
    Aggregate,
    Tap,
    Query,
    Start,
    Register,
    Rebuild,
    Diagnostic,
}

struct Fake<'a> {
    gone_status: Cell<i32>,
    fail: Cell<Failure>,
    format: Cell<TapFormat>,
    change_during_query: Cell<Option<&'a TapSession>>,
    change_during_processes: Cell<Option<&'a TapSession>>,
    unregister_calls: Cell<u32>,
    selected: RefCell<Vec<native::ProcessIdentity>>,
    rebuild_calls: Cell<u32>,
    stop_calls: Cell<u32>,
    device: Cell<native::DeviceLease>,
    requested_device: Cell<Option<u32>>,
    diagnostic_selection: RefCell<Option<native::Selection>>,
    diagnostic_live: Cell<bool>,
    diagnostic_audio: Cell<bool>,
    diagnostic_starts: Cell<u32>,
    diagnostic_retires: Cell<u32>,
    diagnostic_preparation: Cell<std::time::Duration>,
    resume_during_diagnostic: Cell<Option<&'a TapSession>>,
    callback_during_start: Cell<Option<&'a TapSession>>,
}

impl Default for Fake<'_> {
    fn default() -> Self {
        Self {
            gone_status: Cell::new(0),
            fail: Cell::new(Failure::None),
            format: Cell::new(format()),
            change_during_query: Cell::new(None),
            change_during_processes: Cell::new(None),
            unregister_calls: Cell::new(0),
            selected: RefCell::new(Vec::new()),
            rebuild_calls: Cell::new(0),
            stop_calls: Cell::new(0),
            device: Cell::new(native::DeviceLease {
                object: 7,
                stream: 70,
            }),
            requested_device: Cell::new(None),
            diagnostic_selection: RefCell::new(None),
            diagnostic_live: Cell::new(false),
            diagnostic_audio: Cell::new(false),
            diagnostic_starts: Cell::new(0),
            diagnostic_retires: Cell::new(0),
            diagnostic_preparation: Cell::new(std::time::Duration::ZERO),
            resume_during_diagnostic: Cell::new(None),
            callback_during_start: Cell::new(None),
        }
    }
}

impl Fake<'_> {
    fn result(&self, boundary: Failure) -> Result<(), TapError> {
        if self.gone_status.get() != 0
            && matches!(
                boundary,
                Failure::Stop
                    | Failure::Unregister
                    | Failure::Listener
                    | Failure::Aggregate
                    | Failure::Tap
            )
        {
            return TapError::status(self.gone_status.get());
        }
        if self.fail.get() == boundary {
            TapError::status(-1)
        } else {
            Ok(())
        }
    }
}

impl Native for Fake<'_> {
    fn device(&self, _: &std::ffi::CStr) -> Result<native::DeviceLease, TapError> {
        Ok(self.device.get())
    }
    fn processes(
        &self,
        _: &std::ffi::CStr,
        target_device: Option<u32>,
    ) -> Result<Vec<native::ProcessIdentity>, TapError> {
        self.requested_device.set(target_device);
        if let Some(session) = self.change_during_processes.take() {
            session.inventory_epoch.fetch_add(1, Ordering::Release);
            session.lifecycle.changed();
        }
        Ok(self.selected.borrow().clone())
    }
    fn rebuild(&self, session: &TapSession, _: &native::Selection) -> Result<(), TapError> {
        self.rebuild_calls.set(self.rebuild_calls.get() + 1);
        self.result(Failure::Rebuild)?;
        let mut resources = session.resources.get();
        resources.tap = 1;
        resources.aggregate = 2;
        resources.listeners.format = true;
        session.resources.set(resources);
        Ok(())
    }
    fn format(&self, _: u32) -> Result<TapFormat, TapError> {
        if let Some(session) = self.change_during_query.take() {
            // SAFETY: fixture session is borrowed through the complete query.
            session.lifecycle.changed();
        }
        self.result(Failure::Query)?;
        Ok(self.format.get())
    }
    fn start(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        if let Some(session) = self.callback_during_start.take() {
            callback(session, &mut [0.75; 64], None);
        }
        self.result(Failure::Start)
    }
    fn stop(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        self.stop_calls.set(self.stop_calls.get() + 1);
        self.result(Failure::Stop)
    }
    fn register(&self, session: &TapSession) -> Result<(), TapError> {
        session.adopt_registration(
            std::ptr::dangling_mut(),
            if self.fail.get() == Failure::Register {
                -1
            } else {
                0
            },
        )
    }
    fn unregister(&self, _: u32, _: *mut c_void) -> Result<(), TapError> {
        self.unregister_calls.set(self.unregister_calls.get() + 1);
        self.result(Failure::Unregister)
    }
    fn remove_listener(&self, _: u32, _: &TapSession) -> Result<(), TapError> {
        self.result(Failure::Listener)
    }
    fn remove_inventory_listener(&self, _: &TapSession) -> Result<(), TapError> {
        self.result(Failure::InventoryListener)
    }
    fn remove_service_listener(&self, _: &TapSession) -> Result<(), TapError> {
        self.result(Failure::ServiceListener)
    }
    fn install_system_listeners(&self, session: &TapSession) -> Result<(), TapError> {
        let mut resources = session.resources.get();
        resources.listeners.service = true;
        session.resources.set(resources);
        self.result(Failure::SystemInstall)?;
        resources.listeners.inventory = session
            .selection
            .borrow()
            .as_ref()
            .is_some_and(|selection| selection.fallback);
        session.resources.set(resources);
        Ok(())
    }
    fn destroy_aggregate(&self, _: u32) -> Result<(), TapError> {
        self.result(Failure::Aggregate)
    }
    fn destroy_tap(&self, _: u32) -> Result<(), TapError> {
        self.result(Failure::Tap)
    }
    fn diagnostic(
        &self,
        selection: &native::Selection,
        observed_at: std::time::Instant,
    ) -> Result<crate::watchdog::Diagnostic, TapError> {
        self.diagnostic_starts.set(self.diagnostic_starts.get() + 1);
        self.result(Failure::Diagnostic)?;
        assert!(!self.diagnostic_live.replace(true));
        self.diagnostic_selection.replace(Some(selection.clone()));
        Ok(crate::watchdog::Diagnostic {
            session: ptr::dangling_mut(),
            preparation_started: observed_at,
            preparation_time: self.diagnostic_preparation.get(),
        })
    }
    fn diagnostic_has_audio(&self, _: &crate::watchdog::Diagnostic) -> bool {
        assert!(self.diagnostic_live.get());
        if let Some(session) = self.resume_during_diagnostic.take() {
            callback(session, &mut [0.5, -0.5], None);
        }
        self.diagnostic_audio.get()
    }
    fn retire_diagnostic(&self, _: crate::watchdog::Diagnostic) {
        assert!(self.diagnostic_live.replace(false));
        self.diagnostic_retires
            .set(self.diagnostic_retires.get() + 1);
    }
}

#[test]
fn partial_registration_is_adopted_before_error_and_success_requires_an_identifier() {
    let session = fixture();
    let identifier = std::ptr::dangling_mut();
    assert_eq!(
        session.adopt_registration(identifier, -9),
        Err(TapError {
            code: 4,
            os_status: -9
        })
    );
    assert_eq!(session.resources.get().proc, identifier);
    assert_eq!(
        session
            .adopt_registration(ptr::null_mut(), 0)
            .expect_err("null proc")
            .code,
        4
    );
    assert!(session.resources.get().proc.is_null());
    assert!(session.reclaim(&Fake::default()));
}

#[test]
fn native_property_errors_release_owned_outputs_without_inspecting_them() {
    let value = std::ptr::dangling();
    for (status, size) in [(-1, 8), (0, 4), (0, 8)] {
        let compared = Cell::new(0);
        let released = Cell::new(0);
        let matched = native::finish_property(
            (status, size, value),
            |actual| {
                assert_eq!(actual, value);
                compared.set(compared.get() + 1);
                true
            },
            |actual| {
                assert_eq!(actual, value);
                released.set(released.get() + 1);
            },
        );
        assert_eq!(matched, status == 0 && size == 8);
        assert_eq!(compared.get(), u32::from(matched));
        assert_eq!(released.get(), 1);
    }
    assert!(!native::finish_property(
        (0, 8, ptr::null()),
        |_| panic!("null compared"),
        |_| panic!("null released")
    ));
}

#[test]
fn cleanup_queue_failure_is_rejected_before_session_publication() {
    assert_eq!(
        native::checked_queue(ptr::null_mut)
            .expect_err("allocation failure")
            .code,
        6
    );
    let value = ptr::dangling_mut::<c_void>();
    assert_eq!(native::checked_queue(|| value), Ok(value as usize));
}

#[test]
fn public_capture_boundary_rejects_null_and_invalid_startup_without_native_objects() {
    let mut error = TapError::default();
    // SAFETY: null and invalid startup are explicit API inputs; no dangling pointer
    // is fabricated. Error/result buffers remain exclusive and correctly sized.
    unsafe {
        assert!(mr_process_tap_create(ptr::null(), &raw mut error).is_null());
        assert_eq!(error.code, 1);
        assert_eq!(mr_process_tap_start(ptr::null_mut()).code, 1);
        assert_eq!(mr_process_tap_stop(ptr::null_mut()).code, 1);
        assert_eq!(mr_process_tap_refresh_format(ptr::null_mut()).code, 1);
        assert_eq!(mr_process_tap_read(ptr::null_mut(), ptr::null_mut(), 3), 0);
        assert_eq!(
            mr_process_tap_take_peaks(ptr::null_mut(), ptr::null_mut(), 3),
            0
        );
        assert_eq!(mr_process_tap_stats(ptr::null()).channels, 0);
        mr_process_tap_destroy(ptr::null_mut());
        for capacity in [0, 255, 257, (1 << 20) + 1] {
            let config = TapConfig {
                bundle_id: c"io.mixrelay.fixture".as_ptr(),
                mute_while_captured: false,
                ring_capacity_frames: capacity,
                target_device_uid: ptr::null(),
                system_audio: false,
                excluded_bundle_ids: ptr::null(),
                excluded_bundle_count: 0,
            };
            assert!(mr_process_tap_create(&raw const config, &raw mut error).is_null());
            assert_eq!(error.code, 1);
        }
    }
}

#[test]
fn mute_requires_one_exact_owned_endpoint_uid_before_native_admission() {
    assert!(native::owned_endpoint_uid(
        c"io.mixrelay.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0"
    ));
    for uid in [
        c"io.mixrelay.audio.endpoint.default",
        c"io.mixrelay.audio.endpoint.17E59D7342C14F11A8B46C2D913357E0",
        c"io.mixrelay.audio.endpoint.00000000000000000000000000000000",
        c"com.other.audio.endpoint.17e59d7342c14f11a8b46c2d913357e0",
    ] {
        assert!(!native::owned_endpoint_uid(uid));
    }
    let mut error = TapError::default();
    let config = TapConfig {
        bundle_id: c"io.mixrelay.fixture".as_ptr(),
        mute_while_captured: true,
        ring_capacity_frames: 256,
        target_device_uid: ptr::null(),
        system_audio: false,
        excluded_bundle_ids: ptr::null(),
        excluded_bundle_count: 0,
    };
    // SAFETY: invalid config and exclusive error output remain live for the call.
    assert!(unsafe { mr_process_tap_create(&raw const config, &raw mut error) }.is_null());
    assert_eq!(error.code, 1);
}

#[test]
fn changed_owned_device_lease_invalidates_muted_capture_before_retrack() {
    let session = muted_fixture();
    let native = Fake::default();
    native.device.set(native::DeviceLease {
        object: 8,
        stream: 80,
    });
    assert_eq!(session.refresh(&native), Err(TapError::code(9)));
    assert!(!session.allowed());
    assert_eq!(native.requested_device.get(), None);
}

#[test]
fn changed_owned_stream_lease_invalidates_muted_capture() {
    let session = muted_fixture();
    let native = Fake::default();
    native.device.set(native::DeviceLease {
        object: 7,
        stream: 71,
    });
    assert_eq!(session.refresh(&native), Err(TapError::code(9)));
    assert!(!session.allowed());
}

#[test]
fn matching_owned_device_and_process_snapshot_keep_capture_admitted() {
    let session = muted_fixture();
    let native = Fake::default();
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 7, pid: 71 }]);
    assert_eq!(session.refresh(&native), Ok(()));
    assert!(session.allowed());
    assert_eq!(native.requested_device.get(), Some(7));
}

#[test]
fn scoped_mute_process_change_revokes_lease_instead_of_retracking() {
    for candidate in [vec![], vec![native::ProcessIdentity { object: 8, pid: 72 }]] {
        let session = muted_fixture();
        let native = Fake::default();
        native.selected.replace(candidate);
        assert_eq!(session.refresh(&native), Err(TapError::code(9)));
        assert!(!session.allowed());
        assert_eq!(
            session.selection.borrow().as_ref().unwrap().selected,
            vec![native::ProcessIdentity { object: 7, pid: 71 }]
        );
    }
}

#[repr(C)]
struct Buffer {
    count: u32,
    part: AudioBuffer,
}

fn callback(session: &TapSession, samples: &mut [f32], time: Option<&native::Timestamp>) {
    let buffer = Buffer {
        count: 1,
        part: AudioBuffer {
            channels: 2,
            byte_size: u32::try_from(size_of_val(samples)).expect("small fixture"),
            data: samples.as_mut_ptr(),
        },
    };
    // SAFETY: fixture supplies one complete stereo buffer and retains the session/time;
    // callbacks and control are serialized except the separately tested atomic guards.
    unsafe {
        ffi::io_proc(
            2,
            ptr::null(),
            ptr::from_ref(&buffer).cast::<AudioBufferList>(),
            time.map_or(ptr::null(), ptr::from_ref),
            ptr::null_mut(),
            ptr::null(),
            ptr::from_ref(session).cast_mut().cast(),
        )
    };
}

#[test]
fn callback_coordinates_use_the_refreshed_native_rate() {
    let session = fixture();
    let timebase = mixrelay_realtime::Timebase::current();
    session.timebase.set(timebase).unwrap();
    let native = Fake::default();
    native.format.set(TapFormat {
        sample_rate: 44_100.0,
        ..format()
    });
    session.lifecycle.changed();
    session.refresh(&native).unwrap();
    let mut pcm = [0.25; 64];
    let mut timestamp = native::Timestamp::default();
    timestamp.host = timebase.ticks(std::time::Duration::from_secs(1));
    timestamp.rate_scalar = 1.0;
    timestamp.flags = 7;
    let mut output = [0.0; 64];
    let mut times = [0; 32];
    let measured = allocation_counter::measure(|| {
        callback(&session, &mut pcm, Some(&timestamp));
        let stats = session.stats();
        // SAFETY: serialized fixture retains the session and complete output/time spans.
        assert_eq!(
            unsafe {
                ffi::mr_process_tap_read_timed(
                    ptr::from_ref(&session).cast_mut(),
                    output.as_mut_ptr(),
                    times.as_mut_ptr(),
                    32,
                    TapReadGeneration {
                        format: stats.format_generation,
                        clock_breaks: stats.clock_discontinuities,
                    },
                )
            },
            32
        );
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(output.map(f32::to_bits), pcm.map(f32::to_bits));
    assert!(
        times[31].abs_diff(1_000_000_000 + 31 * 1_000_000_000 / 44_100) <= 1,
        "callback kept the initial rate after refresh: {times:?}"
    );
}

#[test]
fn timed_read_preserves_acquisition_coordinates_and_rejects_foreign_generation() {
    let session = fixture();
    session
        .timebase
        .set(mixrelay_realtime::Timebase::current())
        .unwrap();
    let mut pcm = [0.25; 64];
    let host = mixrelay_realtime::Timebase::current().ticks(std::time::Duration::from_secs(1));
    let mut timestamp = native::Timestamp::default();
    timestamp.host = host;
    timestamp.rate_scalar = 1.0;
    timestamp.flags = 7;
    callback(&session, &mut pcm, Some(&timestamp));
    let mut output = [0.0; 64];
    let mut times = [0_u64; 32];
    let measured = allocation_counter::measure(|| unsafe {
        // SAFETY: this serialized fixture retains its session and both complete output spans.
        assert_eq!(
            ffi::mr_process_tap_read_timed(
                ptr::from_ref(&session).cast_mut(),
                output.as_mut_ptr(),
                times.as_mut_ptr(),
                32,
                TapReadGeneration {
                    format: 1,
                    clock_breaks: 1
                }
            ),
            0
        );
        assert_eq!(
            ffi::mr_process_tap_read_timed(
                ptr::from_ref(&session).cast_mut(),
                output.as_mut_ptr(),
                times.as_mut_ptr(),
                32,
                TapReadGeneration {
                    format: 2,
                    clock_breaks: 0
                }
            ),
            0
        );
        assert_eq!(
            ffi::mr_process_tap_read_timed(
                ptr::from_ref(&session).cast_mut(),
                output.as_mut_ptr(),
                times.as_mut_ptr(),
                32,
                TapReadGeneration {
                    format: 1,
                    clock_breaks: 0
                }
            ),
            32
        );
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(output.map(f32::to_bits), pcm.map(f32::to_bits));
    assert!(times[0].abs_diff(1_000_000_000) <= 1);
    assert!(times[31].abs_diff(1_000_000_000 + 31 * 1_000_000_000 / 48_000) <= 1);
}

#[test]
fn each_failed_native_teardown_retains_exact_resources_for_one_thousand_cycles() {
    let native = Fake::default();
    for _ in 0..1000 {
        let session = fixture();
        let ring = session.ring();
        native.fail.set(Failure::Stop);
        assert_eq!(session.stop(&native).expect_err("stop failure").code, 4);
        assert!(session.running.load(Ordering::Acquire));
        native.fail.set(Failure::Unregister);
        assert!(!session.reclaim(&native));
        assert!(!session.resources.get().proc.is_null());
        native.fail.set(Failure::Listener);
        assert!(!session.reclaim(&native));
        assert!(session.resources.get().proc.is_null());
        assert!(session.resources.get().listeners.format);
        native.fail.set(Failure::Aggregate);
        assert!(!session.reclaim(&native));
        assert_eq!(session.resources.get().aggregate, 2);
        native.fail.set(Failure::Tap);
        assert!(!session.reclaim(&native));
        assert_eq!(session.resources.get().tap, 1);
        assert_eq!(session.resources.get().aggregate, 0);
        assert_eq!(session.ring(), ring);
        native.fail.set(Failure::None);
        assert!(session.reclaim(&native));
        assert_eq!(session.resources.get().tap, 0);
    }
}

#[test]
fn callback_and_listener_drain_are_independent_reclamation_requirements() {
    let native = Fake::default();
    let session = fixture();
    session.active_callbacks.store(1, Ordering::Release);
    assert!(!session.reclaim(&native));
    assert_eq!(session.resources.get().aggregate, 2);
    session.active_callbacks.store(0, Ordering::Release);
    session.active_listeners.store(1, Ordering::Release);
    assert!(!session.reclaim(&native));
    assert!(!session.resources.get().listeners.format);
    assert_eq!(session.resources.get().tap, 1);
    session.active_listeners.store(0, Ordering::Release);
    assert!(session.reclaim(&native));
}

#[test]
fn rate_refresh_preserves_storage_and_counters_but_drains_stale_pcm() {
    let session = fixture();
    let native = Fake::default();
    let ring = session.ring();
    let mut samples = [0.25, -0.5, 0.75, -1.0];
    callback(&session, &mut samples, None);
    assert_eq!(session.stats().available_frames, 2);
    native.format.set(TapFormat {
        sample_rate: 44_100.0,
        ..format()
    });
    assert!(session.refresh(&native).is_ok());
    assert_eq!(session.ring(), ring);
    let stats = session.stats();
    assert_eq!(stats.available_frames, 0);
    assert_eq!(stats.captured_frames, 2);
    assert_eq!(stats.sample_rate.to_bits(), 44_100_f64.to_bits());
    assert_eq!(stats.format_generation, 2);
    assert!(stats.running);
    assert!(session.reclaim(&native));
}

#[test]
fn unrelated_inventory_change_keeps_exact_selected_owner_without_hardware_restart() {
    let session = fallback_fixture();
    let native = Fake::default();
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 7, pid: 71 }]);
    let original = session.resources.get();
    session.inventory_epoch.fetch_add(1, Ordering::Release);
    session.lifecycle.changed();
    assert!(!session.allowed());
    assert!(session.refresh(&native).is_ok());
    assert!(session.allowed());
    assert_eq!(session.resources.get().tap, original.tap);
    assert_eq!(session.resources.get().aggregate, original.aggregate);
    assert_eq!(session.resources.get().proc, original.proc);
    assert_eq!(native.stop_calls.get(), 0);
    assert_eq!(native.unregister_calls.get(), 0);
    assert_eq!(native.rebuild_calls.get(), 0);
    assert_eq!(session.stats().format_generation, 1);
    assert!(session.reclaim(&native));
}

#[test]
fn inventory_change_during_discovery_cannot_certify_stale_selection() {
    let session = fallback_fixture();
    let native = Fake::default();
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 7, pid: 71 }]);
    native.change_during_processes.set(Some(&session));
    assert_eq!(
        session.refresh(&native).expect_err("racing inventory").code,
        9
    );
    assert!(!session.allowed());
    assert_eq!(native.rebuild_calls.get(), 0);
    assert_eq!(native.stop_calls.get(), 0);
    assert!(session.refresh(&native).is_ok());
    assert!(session.allowed());
    assert_eq!(native.rebuild_calls.get(), 0);
    assert!(session.reclaim(&native));
}

#[test]
fn absent_bundle_stays_silent_then_new_process_epoch_rebuilds_exact_selection() {
    let session = fallback_fixture();
    let native = Fake::default();
    let mut samples = [0.25, -0.25];
    callback(&session, &mut samples, None);
    assert_eq!(session.stats().available_frames, 1);
    session.inventory_epoch.fetch_add(1, Ordering::Release);
    session.lifecycle.changed();
    assert_eq!(session.refresh(&native).expect_err("absent bundle").code, 9);
    assert!(!session.allowed());
    assert_eq!(session.resources.get().tap, 0);
    assert_eq!(session.stats().available_frames, 0);
    assert!(!session.stats().running);
    callback(&session, &mut samples, None);
    assert_eq!(session.stats().available_frames, 0);
    assert_eq!(native.rebuild_calls.get(), 0);

    native.selected.replace(vec![native::ProcessIdentity {
        object: 19,
        pid: 82,
    }]);
    session.inventory_epoch.fetch_add(1, Ordering::Release);
    session.lifecycle.changed();
    assert!(session.refresh(&native).is_ok());
    assert!(session.allowed() && session.stats().running);
    assert_eq!(native.rebuild_calls.get(), 1);
    assert_eq!(session.stats().format_generation, 2);
    assert_eq!(
        session
            .selection
            .borrow()
            .as_ref()
            .expect("selection")
            .selected,
        vec![native::ProcessIdentity {
            object: 19,
            pid: 82
        }]
    );
    assert!(session.reclaim(&native));
}

#[test]
fn reused_process_object_with_new_pid_is_not_certified_as_the_old_owner() {
    let session = fallback_fixture();
    let native = Fake::default();
    native
        .selected
        .replace(vec![native::ProcessIdentity { object: 7, pid: 72 }]);
    session.inventory_epoch.fetch_add(1, Ordering::Release);
    session.lifecycle.changed();
    assert!(session.refresh(&native).is_ok());
    assert_eq!(native.rebuild_calls.get(), 1);
    assert_eq!(native.unregister_calls.get(), 1);
    assert_eq!(
        session
            .selection
            .borrow()
            .as_ref()
            .expect("selection")
            .selected,
        vec![native::ProcessIdentity { object: 7, pid: 72 }]
    );
    assert!(session.reclaim(&native));
}

#[test]
fn invalid_geometry_query_and_failed_restart_never_reopen_delivery() {
    let session = fixture();
    let native = Fake::default();
    native.format.set(TapFormat {
        channels: 1,
        bytes_per_frame: 4,
        bytes_per_packet: 4,
        ..format()
    });
    assert_eq!(
        session
            .refresh(&native)
            .expect_err("incompatible geometry")
            .code,
        5
    );
    assert!(!session.allowed());
    assert_eq!(session.start(&native).expect_err("gated").code, 9);
    assert_eq!(session.format.get().expect("immutable format").channels, 2);
    native.format.set(format());
    native.fail.set(Failure::Query);
    assert_eq!(session.refresh(&native).expect_err("query pending").code, 9);
    native.fail.set(Failure::Register);
    assert_eq!(
        session.refresh(&native).expect_err("register failure").code,
        9
    );
    assert!(!session.allowed());
    native.fail.set(Failure::Start);
    assert_eq!(
        session.refresh(&native).expect_err("restart failure").code,
        9
    );
    assert!(!session.allowed());
    native.fail.set(Failure::None);
    assert!(session.refresh(&native).is_ok());
    assert!(session.allowed() && session.stats().running);
    assert!(session.reclaim(&native));
}

#[test]
fn notification_during_query_rejects_stale_commit_and_retries_fresh_discovery() {
    let session = fixture();
    let native = Fake::default();
    native.format.set(TapFormat {
        sample_rate: 44_100.0,
        ..format()
    });
    native.change_during_query.set(Some(&session));
    assert_eq!(
        session.refresh(&native).expect_err("stale discovery").code,
        9
    );
    assert!(!session.allowed());
    assert!(session.refresh(&native).is_ok());
    assert!(session.allowed());
    assert!(session.reclaim(&native));
}

#[test]
fn whole_callback_retains_clock_credit_for_overflow_and_allocates_nothing() {
    let session = fixture();
    let mut samples = [0.25; 512];
    let measured = allocation_counter::measure(|| {
        callback(&session, &mut samples, None);
        callback(&session, &mut samples, None);
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(session.stats().captured_frames, 256);
    assert_eq!(session.stats().dropped_frames, 256);
    // SAFETY: fixture retains gate and invalidates before the next serialized callback.
    session.lifecycle.changed();
    callback(&session, &mut samples, None);
    assert_eq!(
        session.stats().captured_frames + session.stats().dropped_frames,
        512
    );
    let native = Fake::default();
    assert!(session.refresh(&native).is_ok());
    callback(&session, &mut samples, None);
    assert_eq!(session.stats().captured_frames, 512);
    assert_eq!(session.stats().dropped_frames, 256);
    let mut output = [0.0; 512];
    // SAFETY: sole consumer uses sufficiently sized exclusive output after callback.
    assert_eq!(
        unsafe { mr_audio_ring_read_interleaved(session.ring(), output.as_mut_ptr(), 256) },
        256
    );
    assert_eq!(output.map(f32::to_bits), samples.map(f32::to_bits));
    assert!(session.reclaim(&native));
}

#[test]
fn native_timestamp_continuity_counts_buffer_positions_not_callback_arrival() {
    let session = fixture();
    let mut samples = [0.0; 128];
    let mut time = native::Timestamp::default();
    time.flags = 3;
    time.host = 100;
    callback(&session, &mut samples, Some(&time));
    assert!(session.stats().clock_valid);
    assert_eq!(session.stats().clock_discontinuities, 0);
    time.sample = 128.0;
    time.host = 200;
    callback(&session, &mut samples, Some(&time));
    assert_eq!(session.stats().clock_discontinuities, 1);
    callback(&session, &mut samples, None);
    assert!(!session.stats().clock_valid);
    assert_eq!(session.stats().clock_discontinuities, 2);
    assert!(session.reclaim(&Fake::default()));
}

#[test]
fn each_callback_wakes_the_capture_pump_through_the_session_notifier() {
    // SAFETY: null asks for no session.
    assert!(unsafe { ffi::mr_process_tap_wakeup(ptr::null()) }.is_null());
    let session = fixture();
    // SAFETY: a live fixture session without a notifier.
    assert!(unsafe { ffi::mr_process_tap_wakeup(&raw const session) }.is_null());
    session
        .wakeup
        .set(mixrelay_realtime::Notifier::new().unwrap())
        .unwrap();
    // SAFETY: the accessor borrows the notifier for the live session's lifetime.
    let wakeup = unsafe {
        &*ffi::mr_process_tap_wakeup(&raw const session).cast::<mixrelay_realtime::Notifier>()
    };
    let mut samples = [0.25_f32; 8];
    callback(&session, &mut samples, None);
    assert!(matches!(
        wakeup.wait(std::time::Duration::from_secs(1)),
        mixrelay_realtime::Wake::Notified { .. }
    ));
}

#[test]
fn device_uses_merge_playing_and_recording_per_process_and_device() {
    let mut uses = Vec::new();
    let devices = |uids: &[&str]| uids.iter().map(|uid| (*uid).to_owned()).collect::<Vec<_>>();
    super::push_device_uses(
        &mut uses,
        7,
        "com.example.app",
        &devices(&["a", "b"]),
        &devices(&["b"]),
    );
    super::push_device_uses(&mut uses, 8, "com.example.other", &[], &devices(&["a"]));
    let entry = |process_id, device: &str, playing, recording| super::DeviceUse {
        process_id,
        bundle_id: if process_id == 7 {
            "com.example.app"
        } else {
            "com.example.other"
        }
        .to_owned(),
        device_uid: device.to_owned(),
        playing,
        recording,
    };
    assert_eq!(
        uses,
        [
            entry(7, "a", true, false),
            entry(7, "b", true, true),
            entry(8, "a", false, true)
        ]
    );
    // The live process list is readable without any device or permission.
    assert!(super::device_uses().is_ok());
}
