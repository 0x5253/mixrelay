use super::*;

fn read(session: &TapSession, output: &mut [f32; 64]) -> usize {
    // SAFETY: serialized fixture owns one complete stereo destination and its sole reader.
    unsafe { mr_process_tap_read(ptr::from_ref(session).cast_mut(), output.as_mut_ptr(), 32) }
}

#[test]
fn successful_stop_rejects_late_callbacks_and_withholds_accepted_pcm() {
    let session = fixture();
    let native = Fake::default();
    let mut timestamp = native::Timestamp::default();
    timestamp.flags = 3;
    timestamp.host = 100;
    callback(&session, &mut [0.75; 64], Some(&timestamp));
    assert!(session.stats().clock_valid);
    let before = session.stats().captured_frames;
    session.stop(&native).unwrap();
    let measured = allocation_counter::measure(|| {
        callback(&session, &mut [0.25; 64], Some(&timestamp));
        assert_eq!(read(&session, &mut [0.0; 64]), 0);
    });
    assert_eq!(measured.count_total, 0);
    assert!(!session.stats().running);
    assert!(!session.stats().clock_valid);
    assert_eq!(session.stats().captured_frames, before);
    assert!(session.reclaim(&native));
}

#[test]
fn stopped_capture_restarts_with_only_new_quiet_pcm_and_a_new_generation() {
    let session = fixture();
    let native = Fake::default();
    callback(&session, &mut [0.75; 64], None);
    let generation = session.stats().format_generation;
    session.stop(&native).unwrap();
    session.start(&native).unwrap();
    callback(&session, &mut [0.0; 64], None);
    let mut output = [1.0; 64];
    assert_eq!(read(&session, &mut output), 32);
    assert_eq!(
        output.map(f32::to_bits),
        [0.0_f32.to_bits(); 64],
        "restart replayed PCM accepted before stop"
    );
    assert_ne!(session.stats().format_generation, generation);
    assert_eq!(read(&session, &mut output), 0);
    assert!(session.reclaim(&native));
}

#[test]
fn failed_stop_retains_native_ownership_but_closes_pcm_delivery() {
    let session = fixture();
    let native = Fake::default();
    callback(&session, &mut [0.75; 64], None);
    native.fail.set(Failure::Stop);
    let error = session.stop(&native).unwrap_err();
    assert_eq!(error.os_status, -1);
    assert!(session.stats().running);
    let before = session.stats().captured_frames;
    callback(&session, &mut [0.25; 64], None);
    assert_eq!(session.stats().captured_frames, before);
    assert_eq!(read(&session, &mut [0.0; 64]), 0);
    assert!(!session.resources.get().proc.is_null());
    native.fail.set(Failure::None);
    session.stop(&native).unwrap();
    assert!(session.reclaim(&native));
}

#[test]
fn restart_waits_for_an_entered_callback_before_discarding_or_reopening() {
    let session = fixture();
    let native = Fake::default();
    callback(&session, &mut [0.75; 64], None);
    let active = ffi::Active::enter(&session.active_callbacks);
    session.stop(&native).unwrap();
    // This IOProc passed admission before Stop and was then preempted. Resume
    // its actual ingest while retaining the same active guard used by io_proc.
    let mut late = [0.25; 64];
    let input = Buffer {
        count: 1,
        part: AudioBuffer {
            channels: 2,
            byte_size: u32::try_from(size_of_val(&late)).unwrap(),
            data: late.as_mut_ptr(),
        },
    };
    // SAFETY: one entered callback owns the prepared ring producer and complete
    // native list; the control owner has closed admission but retained storage.
    let accepted = unsafe {
        crate::process_tap_input::ingest(
            session.format.get().unwrap(),
            ptr::from_ref(&input).cast(),
            session.ring(),
            &session.peaks,
            (0, 1e9 / 48_000.0),
        )
    };
    assert_eq!(accepted.written, 32);
    assert_eq!(session.start(&native).unwrap_err().code, 9);
    assert_eq!(session.stats().available_frames, 64);
    drop(active);
    session.start(&native).unwrap();
    assert_eq!(session.stats().available_frames, 0);
    assert!(session.reclaim(&native));
}

#[test]
fn synchronous_first_callback_is_admitted_but_failed_start_pcm_never_escapes() {
    let session = fixture();
    let native = Fake::default();
    session.stop(&native).unwrap();
    native.callback_during_start.set(Some(&session));
    session.start(&native).unwrap();
    let mut output = [0.0; 64];
    assert_eq!(read(&session, &mut output), 32);
    assert_eq!(output.map(f32::to_bits), [0.75_f32.to_bits(); 64]);
    session.stop(&native).unwrap();
    native.fail.set(Failure::Start);
    native.callback_during_start.set(Some(&session));
    assert_eq!(session.start(&native).unwrap_err().os_status, -1);
    assert!(!session.stats().running);
    assert_eq!(read(&session, &mut output), 0);
    native.fail.set(Failure::None);
    session.start(&native).unwrap();
    callback(&session, &mut [0.0; 64], None);
    assert_eq!(read(&session, &mut output), 32);
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 64]);
    assert!(session.reclaim(&native));
}
