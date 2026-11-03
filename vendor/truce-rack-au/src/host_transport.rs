//! Per-render host transport snapshots and Audio Unit transport callbacks.

use objc2_audio_toolbox::kAudioUnitErr_CannotDoInCurrentContext;
use truce_rack_core::transport::TransportInfo;

/// Snapshot queried synchronously by an AU during rendering, with musical positions in beats.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct HostTransport {
    /// False until `process()` sets a transport for the block; the
    /// procs then report `kAudioUnitErr_CannotDoInCurrentContext`.
    valid: bool,
    tempo: f64,
    /// Current position in quarter notes.
    beat: f64,
    /// Downbeat of the current bar, in quarter notes.
    bar_start_beat: f64,
    time_sig_num: f32,
    time_sig_den: u32,
    sample_in_timeline: f64,
    sample_rate: f64,
    playing: bool,
    recording: bool,
    cycling: bool,
}

/// Copy a host [`TransportInfo`] block into the shared
/// [`HostTransport`]. Falls back to the running sample counter when
/// the host didn't report a sample position, and clears `valid`
/// when there's no transport at all.
#[allow(clippy::cast_precision_loss)]
pub(super) fn update_host_transport(
    ht: &mut HostTransport,
    transport: Option<TransportInfo>,
    sample_rate: f64,
    sample_time: f64,
) {
    let Some(t) = transport else {
        ht.valid = false;
        return;
    };
    let (num, den) = t.time_signature.unwrap_or((4, 4));
    ht.valid = true;
    ht.tempo = t.tempo_bpm.unwrap_or(120.0);
    ht.beat = t.song_position_beats.unwrap_or(0.0);
    ht.bar_start_beat = t.bar_start_beats.unwrap_or(0.0);
    ht.time_sig_num = num as f32;
    ht.time_sig_den = den;
    ht.sample_in_timeline = t.song_position_samples.map_or(sample_time, |s| s as f64);
    ht.sample_rate = sample_rate;
    ht.playing = t.playing;
    ht.recording = t.recording;
    ht.cycling = t.loop_active;
}

/// `HostCallback_GetBeatAndTempo` — current beat and tempo.
pub(super) unsafe extern "C-unwind" fn host_beat_and_tempo(
    in_host_user_data: *mut std::ffi::c_void,
    out_beat: *mut f64,
    out_tempo: *mut f64,
) -> i32 {
    let t = unsafe { &*in_host_user_data.cast::<HostTransport>() };
    if !t.valid {
        return kAudioUnitErr_CannotDoInCurrentContext;
    }
    if !out_beat.is_null() {
        unsafe { *out_beat = t.beat };
    }
    if !out_tempo.is_null() {
        unsafe { *out_tempo = t.tempo };
    }
    0
}

/// `HostCallback_GetMusicalTimeLocation` — time signature, the
/// current bar's downbeat, and samples until the next whole beat.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(super) unsafe extern "C-unwind" fn host_musical_time(
    in_host_user_data: *mut std::ffi::c_void,
    out_delta_to_next_beat: *mut u32,
    out_time_sig_num: *mut f32,
    out_time_sig_den: *mut u32,
    out_measure_downbeat: *mut f64,
) -> i32 {
    let t = unsafe { &*in_host_user_data.cast::<HostTransport>() };
    if !t.valid {
        return kAudioUnitErr_CannotDoInCurrentContext;
    }
    if !out_delta_to_next_beat.is_null() {
        let frac = t.beat - t.beat.floor();
        let samples_per_beat = if t.tempo > 0.0 {
            t.sample_rate * 60.0 / t.tempo
        } else {
            0.0
        };
        let delta = ((1.0 - frac) * samples_per_beat).round();
        let delta = if delta.is_finite() && delta >= 0.0 {
            delta as u32
        } else {
            0
        };
        unsafe { *out_delta_to_next_beat = delta };
    }
    if !out_time_sig_num.is_null() {
        unsafe { *out_time_sig_num = t.time_sig_num };
    }
    if !out_time_sig_den.is_null() {
        unsafe { *out_time_sig_den = t.time_sig_den };
    }
    if !out_measure_downbeat.is_null() {
        unsafe { *out_measure_downbeat = t.bar_start_beat };
    }
    0
}

/// `HostCallback_GetTransportState` — play / cycle state and the
/// current sample position on the host timeline.
pub(super) unsafe extern "C-unwind" fn host_transport_state(
    in_host_user_data: *mut std::ffi::c_void,
    out_is_playing: *mut u8,
    out_transport_changed: *mut u8,
    out_sample_in_timeline: *mut f64,
    out_is_cycling: *mut u8,
    out_cycle_start: *mut f64,
    out_cycle_end: *mut f64,
) -> i32 {
    let t = unsafe { &*in_host_user_data.cast::<HostTransport>() };
    if !t.valid {
        return kAudioUnitErr_CannotDoInCurrentContext;
    }
    if !out_is_playing.is_null() {
        unsafe { *out_is_playing = u8::from(t.playing) };
    }
    if !out_transport_changed.is_null() {
        unsafe { *out_transport_changed = 0 };
    }
    if !out_sample_in_timeline.is_null() {
        unsafe { *out_sample_in_timeline = t.sample_in_timeline };
    }
    if !out_is_cycling.is_null() {
        unsafe { *out_is_cycling = u8::from(t.cycling) };
    }
    if !out_cycle_start.is_null() {
        unsafe { *out_cycle_start = 0.0 };
    }
    if !out_cycle_end.is_null() {
        unsafe { *out_cycle_end = 0.0 };
    }
    0
}

/// `HostCallback_GetTransportState2` — same as
/// [`host_transport_state`] plus the record-enable flag.
pub(super) unsafe extern "C-unwind" fn host_transport_state2(
    in_host_user_data: *mut std::ffi::c_void,
    out_is_playing: *mut u8,
    out_is_recording: *mut u8,
    out_transport_changed: *mut u8,
    out_sample_in_timeline: *mut f64,
    out_is_cycling: *mut u8,
    out_cycle_start: *mut f64,
    out_cycle_end: *mut f64,
) -> i32 {
    let t = unsafe { &*in_host_user_data.cast::<HostTransport>() };
    if !t.valid {
        return kAudioUnitErr_CannotDoInCurrentContext;
    }
    if !out_is_playing.is_null() {
        unsafe { *out_is_playing = u8::from(t.playing) };
    }
    if !out_is_recording.is_null() {
        unsafe { *out_is_recording = u8::from(t.recording) };
    }
    if !out_transport_changed.is_null() {
        unsafe { *out_transport_changed = 0 };
    }
    if !out_sample_in_timeline.is_null() {
        unsafe { *out_sample_in_timeline = t.sample_in_timeline };
    }
    if !out_is_cycling.is_null() {
        unsafe { *out_is_cycling = u8::from(t.cycling) };
    }
    if !out_cycle_start.is_null() {
        unsafe { *out_cycle_start = 0.0 };
    }
    if !out_cycle_end.is_null() {
        unsafe { *out_cycle_end = 0.0 };
    }
    0
}

// SAFETY: AU v2 instances are not movable across audio rendering
// threads while active, but we serialize all access through
