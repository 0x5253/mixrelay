use super::DeviceIo;
use crate::{
    audio_buffer::{self, AudioBufferList},
    callback_clock::{mr_callback_clock_discontinuities, mr_callback_clock_record_values},
    device_buffers::{native_frames, native_transfer},
};
use mixrelay_pcm_ring::{mr_pcm_ring_discard, mr_pcm_ring_read_available};
use std::{
    ffi::c_void,
    ptr,
    sync::atomic::Ordering::{Acquire, Relaxed, Release},
};

#[repr(C)]
#[derive(Default)]
pub(super) struct Timestamp {
    pub(super) sample: f64,
    pub(super) host: u64,
    pub(super) rate_scalar: f64,
    pub(super) word_clock: u64,
    pub(super) smpte: [u8; 24],
    pub(super) flags: u32,
    pub(super) reserved: u32,
}

/// `CoreAudio`'s registered callback, also exercised by the native PCM benchmark.
/// Each call contributes silence to every supplied output before validating its
/// selected layout. Layout failure or a format notification latches silence until reconnect.
/// # Safety
/// `CoreAudio` retains the exact `DeviceIo` context until unregister acknowledges drain.
/// Calls for this registration are serialized. Native lists contain their complete
/// trailing metadata arrays and advertised PCM extents; output storage is writable.
/// The worker and graph each own only their opposite queue cursors. Timestamp
/// pointers are null or native `AudioTimeStamp` values retained for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_device_io_process(
    _device: u32,
    now: *const c_void,
    input: *const c_void,
    input_time: *const c_void,
    output: *mut c_void,
    output_time: *const c_void,
    context: *mut c_void,
) -> i32 {
    let entered = mixrelay_realtime::host_time();
    // SAFETY: callback owns advertised output PCM; silence precedes all early returns.
    unsafe { silence(output.cast()) };
    // SAFETY: retained stable context, with field-level cells for concurrent roles.
    let Some(io) = (unsafe { context.cast::<DeviceIo>().as_ref() }) else {
        return 0;
    };
    let _execution = io.execution.enter(entered, mixrelay_realtime::host_time);
    if io.progress.invalid.load(Acquire) {
        return 0;
    }
    // SAFETY: only the serialized IOProc borrows Callback while registered.
    let callback = unsafe { &mut *io.callback.get() };
    if invalid(io, callback) {
        return 0;
    }
    let buffers = if io.selection.input {
        input
    } else {
        output.cast_const()
    }
    .cast::<AudioBufferList>();
    // Empty/missing callback directions are not a format failure, matching CoreAudio duplex calls.
    if unsafe { audio_buffer::buffers(buffers, callback.selection.limit as usize) }.is_none() {
        return 0;
    }
    // SAFETY: full native metadata extent retained for this callback.
    let frames = unsafe { native_frames(buffers, callback.selection, &callback.formats) };
    if frames == 0 {
        io.progress.invalid.store(true, Release);
        return 0;
    }
    // SAFETY: selected native timestamp and serialized clock writer belong to this callback.
    unsafe {
        record_clock(
            io,
            callback,
            if io.selection.input {
                input_time
            } else {
                output_time
            },
            frames,
            now,
        );
    };
    io.progress.native_quantum.store(frames, Relaxed);
    // A format listener can fire during metadata/clock handling. Recheck immediately before
    // exposing native storage to the cached-codec hook or the F32 queue conversion.
    if invalid(io, callback) {
        return 0;
    }
    // SAFETY: hook/context installed before registration and retained by engine owner;
    // borrowed native buffers never escape the synchronous render dispatch.
    // SAFETY: OS retains both complete timestamps for this synchronous callback.
    let timing = unsafe {
        super::IoTiming {
            now: copy_time(now),
            input: copy_time(input_time),
            output: copy_time(output_time),
        }
    };
    record_timing(io, callback, timing, frames);
    let rendered = callback.formats.all_float()
        && callback.hook.is_some_and(|hook| unsafe {
            hook(callback.context, input, output, frames, &raw const timing)
        });
    // SAFETY: this callback owns native transfer; each hook stage borrows only
    // its SRC cursors after the preceding native/scratch borrow has ended.
    let processed = rendered || unsafe { process_pcm(io, callback, buffers, frames) };
    if invalid(io, callback) {
        // A device notification can arrive after the hook has written this block.
        // SAFETY: the callback still owns the full writable native output extent.
        unsafe { silence(output.cast()) };
        return 0;
    }
    if !processed {
        return 0;
    }
    io.progress.ticks.fetch_add(1, Relaxed);
    io.progress
        .native_frames
        .fetch_add(u64::from(frames), Release);
    if let Some(signal) = &io.credit_signal {
        signal();
    }
    0
}

fn invalid(io: &DeviceIo, callback: &super::Callback) -> bool {
    if io.progress.invalid.load(Acquire) {
        return true;
    }
    if callback.watch_invalid.load(Acquire) {
        // Publish callback-observed listener invalidation without exposing its
        // registration-owned Arc or mutable callback state to the graph role.
        // Only preparation after acknowledged drain can clear this stable latch.
        io.progress.invalid.store(true, Release);
        return true;
    }
    false
}

fn record_timing(
    io: &DeviceIo,
    callback: &mut super::Callback,
    timing: super::IoTiming,
    frames: u32,
) {
    callback.acquisition_nanos = if timing.input.flags & 3 == 3 {
        callback.timebase.nanoseconds(timing.input.host_time)
    } else {
        0
    };
    let ticks = callback.frame_ticks;
    if ticks.is_finite() && ticks > 0.0 {
        // Convert a large fixed tick interval, retaining fractional ticks per frame.
        #[expect(
            clippy::cast_precision_loss,
            reason = "fixed one-million-tick timebase conversion"
        )]
        let period = ticks * callback.timebase.nanoseconds(1_000_000) as f64 / 1_000_000.0;
        callback.frame_nanos = period;
        io.progress.frame_nanos.store(period.to_bits(), Release);
    }
    io.measurements.callback(timing, frames, false);
    if timing.now.flags & 2 != 0 {
        io.progress
            .callback_host
            .store(block_host(timing.now.host_time, callback, frames), Release);
    }
}

// SAFETY: the serialized callback owns native PCM/scratch. Hook stages may use
// only opposite queue cursors and cannot retain any native or scratch borrow.
unsafe fn process_pcm(
    io: &DeviceIo,
    callback: &mut super::Callback,
    buffers: *const AudioBufferList,
    frames: u32,
) -> bool {
    // A direct hook may have rejected its block after a native notification.
    // Recheck before the fallback can admit or pump an old-format PCM packet.
    if invalid(io, callback) {
        return false;
    }
    if !io.selection.input {
        // Retire old output before the hook can produce the new epoch's samples.
        unsafe { prepare_output(io, callback) };
    }
    // Admission/reset runs first, so OFF and unprepared formats never enqueue
    // capture PCM or grow a native backlog while waiting for a new converter.
    let admitted = callback
        .pcm_hook
        .is_none_or(|hook| unsafe { hook(callback.pcm_context, super::PcmPhase::Prepare as u32) });
    if invalid(io, callback) {
        return false;
    }
    if admitted {
        // SAFETY: transfer completes all native/scratch borrows before Captured.
        if !unsafe { transfer(io, callback, buffers, frames) } {
            io.progress.invalid.store(true, Release);
            return false;
        }
        if io.selection.input {
            if invalid(io, callback) {
                return false;
            }
            // Convert this packet now, rather than leaving every captured frame
            // in the native queue until the next full hardware callback.
            if callback.pcm_hook.is_some_and(|hook| unsafe {
                !hook(callback.pcm_context, super::PcmPhase::Captured as u32)
            }) {
                io.measurements.faults.fetch_add(1, Release);
            }
        }
    }
    true
}

/// The mean-frame host label for queue transfers performed by this callback,
/// `now + (frames - 1) / 2` frames. It measures passage through this queue boundary;
/// it does not assign an acquisition timestamp to previously buffered PCM.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite nonnegative ticks within one callback"
)]
fn block_host(now: u64, callback: &super::Callback, frames: u32) -> u64 {
    now + (callback.frame_ticks * f64::from(frames - 1) / 2.0) as u64
}

// SAFETY: pointer is null or a complete AudioTimeStamp retained by CoreAudio.
unsafe fn copy_time(pointer: *const c_void) -> super::IoTimestamp {
    unsafe { pointer.cast::<Timestamp>().as_ref() }.map_or_else(
        super::IoTimestamp::default,
        |time| super::IoTimestamp {
            sample_time: time.sample,
            host_time: time.host,
            rate_scalar: time.rate_scalar,
            flags: time.flags,
        },
    )
}

// SAFETY: only this serialized native consumer changes the output ring cursor.
unsafe fn prepare_output(io: &DeviceIo, callback: &mut super::Callback) {
    let epoch = io.progress.output_epoch.load(Acquire);
    if callback.prime || callback.epoch != epoch {
        unsafe { mr_pcm_ring_discard(ptr::from_ref(&io.native_ring).cast_mut()) };
        io.measurements.faults.fetch_add(1, Release);
        callback.prime = false;
        callback.epoch = epoch;
    }
}

// SAFETY: invoked only by the serialized native callback, with full validated
// metadata, callback-owned scratch and direction-specific ring cursor ownership.
unsafe fn transfer(
    io: &DeviceIo,
    callback: &mut super::Callback,
    buffers: *const AudioBufferList,
    frames: u32,
) -> bool {
    let ring = ptr::from_ref(&io.native_ring).cast_mut();
    let host = io.progress.callback_host.load(Relaxed);
    if !io.selection.input {
        // SAFETY: sole native consumer and callback-owned, full-width scratch.
        let read =
            unsafe { mr_pcm_ring_read_available(ring, callback.scratch.as_mut_ptr(), frames) };
        if read < frames {
            io.measurements.faults.fetch_add(1, Release);
        }
        io.consumed(false, read, host);
    }
    // SAFETY: scratch is disjoint from native PCM; callback owns the selected output
    // or immutable input. Geometry was admitted for at most MAX_FRAMES*channels.
    if !unsafe {
        native_transfer(
            buffers,
            callback.selection,
            &callback.formats,
            callback.scratch.as_mut_ptr(),
            io.selection.input,
        )
    } {
        return false;
    }
    if io.selection.input {
        // SAFETY: IOProc is native input ring's sole producer; scratch remains live.
        let written = unsafe {
            mixrelay_pcm_ring::write_observed(ring, callback.scratch.as_ptr(), frames, |cursor| {
                io.native_times.range(
                    cursor,
                    frames as usize,
                    callback.acquisition_nanos,
                    callback.frame_nanos,
                );
            })
        };
        io.produced(false, written, host);
    }

    true
}
// SAFETY: CoreAudio supplies a full list and writable advertised output extents.
unsafe fn silence(output: *mut AudioBufferList) {
    if output.is_null() {
        return;
    }
    // SAFETY: read trailing metadata from the original native pointer, not a
    // sized-header reference that would narrow the allocation provenance.
    let count = unsafe { ptr::addr_of!((*output).count).read() } as usize;
    // SAFETY: the OS callback contract owns the entire declared metadata array.
    if let Some(buffers) = unsafe { audio_buffer::buffers(output, count) } {
        for buffer in buffers {
            if !buffer.data.is_null() {
                // SAFETY: byte writes need no f32 alignment; even a malformed
                // format is silenced over its actual native writable byte extent.
                unsafe { ptr::write_bytes(buffer.data.cast::<u8>(), 0, buffer.byte_size as usize) };
            }
        }
    }
}

// SAFETY: timestamp is null or retained native ABI storage; this is the sole clock writer.
unsafe fn record_clock(
    io: &DeviceIo,
    callback: &mut super::Callback,
    timestamp: *const c_void,
    frames: u32,
    now: *const c_void,
) {
    io.execution.observe_clock();
    // SAFETY: exact CoreAudio AudioTimeStamp ABI, asserted below.
    let timestamp = unsafe { timestamp.cast::<Timestamp>().as_ref() };
    let (sample, host, valid) = timestamp.map_or((0.0, 0, false), |time| {
        (time.sample, time.host, time.flags & 3 == 3)
    });
    // SAFETY: the native registration retains this clock and serializes its writer.
    let before = unsafe { mr_callback_clock_discontinuities(ptr::from_ref(&io.clock)) };
    // SAFETY: callback serialized by registration; clock writer uses its own UnsafeCell.
    unsafe {
        mr_callback_clock_record_values(
            ptr::from_ref(&io.clock).cast_mut(),
            sample,
            host,
            valid,
            u64::from(frames),
        );
    };
    let after = unsafe { mr_callback_clock_discontinuities(ptr::from_ref(&io.clock)) };
    if after != before {
        // SAFETY: CoreAudio retains the same complete timestamp ABI as input/output time.
        let now = unsafe { now.cast::<Timestamp>().as_ref() };
        let (previous_sample, previous_host, previous_frames) =
            callback.previous_timestamp.unwrap_or((f64::NAN, 0, 0));
        let (previous_enter, previous_exit, callback_enter) = io.execution.coordinates();
        io.clock_evidence.publish(super::ClockBreak {
            count: after,
            expected_sample: previous_sample + f64::from(previous_frames),
            actual_sample: sample,
            previous_host,
            actual_host: host,
            flags: timestamp.map_or(0, |time| u64::from(time.flags)),
            callback_host: now.map_or(0, |time| time.host),
            callback_flags: now.map_or(0, |time| u64::from(time.flags)),
            previous_enter,
            previous_exit,
            callback_enter,
        });
    } else if let Some((previous_sample, previous_host, _)) = callback.previous_timestamp
        && valid
        && previous_host != 0
        && sample > previous_sample
    {
        #[allow(clippy::cast_precision_loss)]
        let elapsed = host.saturating_sub(previous_host) as f64;
        callback.frame_ticks = elapsed / (sample - previous_sample);
    }
    callback.previous_timestamp = Some((sample, host, frames));
}

const _: () = assert!(size_of::<Timestamp>() == 64 && std::mem::offset_of!(Timestamp, flags) == 56);
