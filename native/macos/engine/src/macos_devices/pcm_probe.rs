#![cfg_attr(
    feature = "native-lab",
    expect(
        clippy::expect_used,
        clippy::unwrap_used,
        reason = "explicit installed Driver diagnostics fail immediately when their native contract is violated"
    )
)]
//! Native client PCM continuity. The default test loops one simulated duplex device back
//! through the production callbacks; the opt-in soak uses the installed Driver with ordinary
//! `CoreAudio` admission and native codecs. Explicit UID/width/format select an already
//! configured device; no setting is written.
#[cfg(feature = "native-lab")]
use crate::macos_devices::host::CoreAudio;
#[cfg(test)]
use crate::macos_devices::simulated::{CAPTURED, DEFAULT_BUFFER, SimulatedHost};
use crate::macos_devices::{
    device::Device, ffi, host::Host, native::Native, spec::Spec, worker_retirement,
};
use mixrelay_audio_graph::DeviceSelector;
#[cfg(feature = "native-lab")]
use mixrelay_audio_io::{NativeSampleFormat, NativeStreamFormat};
use std::{
    cell::UnsafeCell,
    ffi::c_void,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const RATE: u32 = 48_000;
const PRIMING: u32 = RATE;
#[cfg(test)]
const PHASE_FRAMES: u64 = 240_000;
const FRAME_LIMIT: u32 = 1 << 20;
const MAX_FRAMES: usize = 4096;
/// audio return path cycles that overlap the other client's retirement; a fixed count keeps the
/// marker budget independent of how long its worker takes to unregister.
#[cfg(test)]
const DETACH_CYCLES: u32 = 8;

// Two 10-bit words identify every frame before FRAME_LIMIT. Mixing the low word
// into every lane makes each lane change every frame. Disjoint channel bands
// preserve lane identity in every frame. Levels are exact PCM16 binary fractions
// in [-1, 1), also exact in PCM24/32 and Float32.
fn sample_word(word: u16, lane: u16) -> f32 {
    f32::from(lane) / 32.0 - 1.0 + f32::from(word) / 32768.0
}

fn marker(frame: u32, lane: u16) -> f32 {
    let [a, b, c, _] = frame.to_le_bytes();
    let low = u32::from(u16::from_le_bytes([a, b]) & 1023);
    let high = u32::from(u16::from_le_bytes([b, c]) >> 2) & 1023;
    let word = match lane {
        0 => low,
        1 => high + low * 17,
        _ => high + low * (u32::from(lane) * 2 + 1) + u32::from(lane) * 37,
    };
    let [a, b, _, _] = word.to_le_bytes();
    sample_word(u16::from_le_bytes([a, b]) & 1023, lane)
}

fn decode_word(sample: f32, lane: u16) -> Option<u16> {
    if !sample.is_finite() {
        return None;
    }
    let (mut first, mut end) = (0_u16, 1024_u16);
    while first < end {
        let middle = first + (end - first) / 2;
        let expected = sample_word(middle, lane);
        if sample.to_bits() == expected.to_bits() {
            return Some(middle);
        }
        if sample < expected {
            end = middle;
        } else {
            first = middle + 1;
        }
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct Fault {
    frame: u32,
    lane: u16,
    expected: u32,
    actual: u32,
}

#[derive(Default)]
struct Sequence {
    seen: u32,
    next: Option<u32>,
    looping: bool,
}

impl Sequence {
    fn check(&mut self, samples: &[f32], channels: u16, priming: u32) -> Result<u32, Fault> {
        let mut checked = 0;
        for frame in samples.chunks_exact(usize::from(channels)) {
            self.seen += 1;
            if self.seen <= priming {
                continue;
            }
            let position = if let Some(next) = self.next {
                next
            } else if let (Some(low), Some(mixed)) =
                (decode_word(frame[0], 0), decode_word(frame[1], 1))
            {
                u32::from(low) | (u32::from(mixed.wrapping_sub(low.wrapping_mul(17)) & 1023) << 10)
            } else {
                return Err(Fault {
                    frame: self.seen,
                    lane: 0,
                    expected: 0,
                    actual: frame[0].to_bits(),
                });
            };
            for (lane, sample) in (0..channels).zip(frame) {
                let expected = marker(position, lane).to_bits();
                if (!self.looping && position >= FRAME_LIMIT) || sample.to_bits() != expected {
                    return Err(Fault {
                        frame: position,
                        lane,
                        expected,
                        actual: sample.to_bits(),
                    });
                }
            }
            self.next = Some(if self.looping {
                (position + 1) % FRAME_LIMIT
            } else {
                position + 1
            });
            checked += 1;
        }
        Ok(checked)
    }
}

#[derive(Default)]
struct Statistics {
    produced: AtomicU64,
    checked: AtomicU64,
    faults: AtomicU32,
    bad_frame: AtomicU32,
    bad_lane: AtomicU32,
    expected: AtomicU32,
    actual: AtomicU32,
}

struct Scratch {
    samples: Box<[f32]>,
    sent: u32,
    sequence: Sequence,
    clock_breaks: Option<u64>,
}

struct Probe {
    pointer: *mut c_void,
    channels: u16,
    input: bool,
    scratch: UnsafeCell<Scratch>,
    statistics: Arc<Statistics>,
}

impl Probe {
    fn output(&self, scratch: &mut Scratch, frames: u32) -> bool {
        if !scratch.sequence.looping && scratch.sent + frames >= FRAME_LIMIT {
            self.statistics.faults.fetch_or(1, Ordering::Release);
            return false;
        }
        for (position, frame) in (scratch.sent..scratch.sent + frames)
            .zip(scratch.samples.chunks_exact_mut(usize::from(self.channels)))
        {
            for (lane, sample) in (0..self.channels).zip(frame) {
                *sample = marker(position, lane);
            }
        }
        // SAFETY: this callback is the native output ring's sole producer; the
        // admitted current quantum owns frames*channels initialized samples.
        if unsafe { ffi::mr_device_io_bridge_write(self.pointer, scratch.samples.as_ptr(), frames) }
            != frames
        {
            self.statistics.faults.fetch_or(2, Ordering::Release);
            return false;
        }
        scratch.sent += frames;
        self.statistics
            .produced
            .store(u64::from(scratch.sent), Ordering::Release);
        true
    }

    fn input(&self, scratch: &mut Scratch, frames: u32) -> bool {
        // SAFETY: this callback is the input native ring's sole consumer. The
        // Captured stage follows this callback's admitted native transfer.
        let read = unsafe {
            ffi::mr_device_io_bridge_read(self.pointer, scratch.samples.as_mut_ptr(), frames)
        };
        if read != frames {
            self.statistics.faults.fetch_or(4, Ordering::Release);
            return false;
        }
        let samples = &scratch.samples[..frames as usize * usize::from(self.channels)];
        match scratch.sequence.check(samples, self.channels, PRIMING) {
            Ok(checked) => {
                if scratch.sequence.next.is_some_and(|next| {
                    u64::from(next) > self.statistics.produced.load(Ordering::Acquire)
                }) {
                    self.statistics.faults.fetch_or(32, Ordering::Release);
                    return false;
                }
                self.statistics
                    .checked
                    .fetch_add(u64::from(checked), Ordering::Release);
                true
            }
            Err(fault) => {
                self.statistics
                    .bad_frame
                    .store(fault.frame, Ordering::Relaxed);
                self.statistics
                    .bad_lane
                    .store(u32::from(fault.lane), Ordering::Relaxed);
                self.statistics
                    .expected
                    .store(fault.expected, Ordering::Relaxed);
                self.statistics
                    .actual
                    .store(fault.actual, Ordering::Relaxed);
                self.statistics.faults.fetch_or(8, Ordering::Release);
                false
            }
        }
    }
}

unsafe extern "C" fn callback(context: *mut c_void, phase: u32) -> bool {
    let Some(phase) = ffi::PcmPhase::from_raw(phase) else {
        return false;
    };
    // SAFETY: Client retains this stable Box until Native destroys and drains
    // the registration, including unwind. CoreAudio serializes this IOProc;
    // only it borrows scratch, while control reads separate atomic statistics.
    let probe = unsafe { &*context.cast::<Probe>() };
    if probe.statistics.faults.load(Ordering::Acquire) != 0 {
        return false;
    }
    if probe.input && phase == ffi::PcmPhase::Prepare {
        return true;
    }
    if !probe.input && phase != ffi::PcmPhase::Prepare {
        return false;
    }
    // SAFETY: retained Native; quantum is an atomic set after actual ASBD buffer
    // validation and before invoking this hook in the same callback.
    let frames = unsafe { ffi::mr_device_io_quantum(probe.pointer, false) };
    if frames == 0 || frames as usize > MAX_FRAMES {
        probe.statistics.faults.fetch_or(16, Ordering::Release);
        return false;
    }
    // SAFETY: the registered native callback exclusively owns this Scratch.
    let scratch = unsafe { &mut *probe.scratch.get() };
    let primed = if probe.input {
        scratch.sequence.seen >= PRIMING
    } else {
        scratch.sent >= PRIMING
    };
    if primed {
        // SAFETY: atomic-only observation of this retained native clock. Its
        // writer recorded the actual callback timestamp before invoking us.
        let breaks = unsafe { ffi::mr_device_io_clock_discontinuities(probe.pointer) };
        if scratch
            .clock_breaks
            .is_some_and(|previous| previous != breaks)
        {
            probe.statistics.faults.fetch_or(64, Ordering::Release);
            return false;
        }
        scratch.clock_breaks = Some(breaks);
    }
    if probe.input {
        probe.input(scratch, frames)
    } else {
        probe.output(scratch, frames)
    }
}

struct Client {
    // Declaration order is the unwind contract: Native unregisters and drains
    // before Rust drops the hook context or its preallocated PCM scratch.
    native: Native,
    probe: Box<Probe>,
}

impl Client {
    #[cfg(test)]
    fn connect(spec: &Spec, statistics: &Arc<Statistics>, host: Arc<dyn Host>) -> Self {
        Self::connect_with_sequence(spec, statistics, false, host)
    }

    #[expect(
        clippy::expect_used,
        reason = "Native PCM fixture setup fails immediately on invalid devices"
    )]
    fn connect_with_sequence(
        spec: &Spec,
        statistics: &Arc<Statistics>,
        looping: bool,
        host: Arc<dyn Host>,
    ) -> Self {
        let native = Native::create(spec, RATE, None, 0, host).expect("prepare native PCM client");
        assert_eq!(
            native.requested_buffer(),
            0,
            "fixture must not write native settings"
        );
        let probe = Box::new(Probe {
            pointer: native.pointer.as_ptr(),
            channels: spec.channels,
            input: spec.input,
            statistics: Arc::clone(statistics),
            scratch: UnsafeCell::new(Scratch {
                samples: vec![0.0; MAX_FRAMES * usize::from(spec.channels)].into_boxed_slice(),
                sent: 0,
                sequence: Sequence {
                    looping,
                    ..Sequence::default()
                },
                clock_breaks: None,
            }),
        });
        let client = Self { native, probe };
        // SAFETY: unregistered sole control owner; Client already owns both
        // Native and the hook Box before connect can start or fail partially.
        unsafe {
            ffi::mr_device_io_pcm_hook(
                client.native.pointer.as_ptr(),
                std::ptr::from_ref(client.probe.as_ref()).cast_mut().cast(),
                callback,
            );
        }
        client
            .native
            .connect(&AtomicBool::new(false))
            .expect("connect native PCM client");
        // SAFETY: sole control-side query of the retained connected owner.
        assert_eq!(
            unsafe { ffi::mr_device_io_native_rate(client.native.pointer.as_ptr()) },
            RATE
        );
        client
    }
}

/// Repeats the exact frame/lane words through wrap. A three-second progress
/// watchdog prevents an entire marker period being silently lost or replayed.
#[cfg(feature = "native-lab")]
pub(crate) fn installed_driver_pcm_soak(
    mut snapshot: impl FnMut() -> mixrelay_driver_core::DriverSnapshot,
) {
    let seconds: u64 = std::env::var("MIXRELAY_TEST_SOAK_SECONDS")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (30..=86_400).contains(&seconds),
        "soak duration must be 30..86400 seconds"
    );
    let output_spec = selection(&snapshot());
    let input_spec = Spec {
        input: true,
        ..output_spec.clone()
    };
    let statistics = Arc::new(Statistics::default());
    let host: Arc<dyn Host> = Arc::new(CoreAudio::default());
    let output = Client::connect_with_sequence(&output_spec, &statistics, true, host.clone());
    let input = Client::connect_with_sequence(&input_spec, &statistics, true, host.clone());
    let other = Device::open(&input_spec, RATE, host).expect("ordinary graph capture client");
    let mut other = Some(other);
    let mut detached_at = None;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds + 10);
    let mut last_progress = started;
    let mut previous = 0;
    let mut last_report = started;
    let target = u64::from(RATE) * seconds;
    while statistics.checked.load(Ordering::Acquire) < target {
        assert_no_pcm_faults(&statistics);
        let now = Instant::now();
        let checked = statistics.checked.load(Ordering::Acquire);
        if checked != previous {
            previous = checked;
            last_progress = now;
        }
        assert!(
            now < deadline && now.duration_since(last_progress) < Duration::from_secs(3),
            "soak deadline/progress failed: checked={checked} target={target}"
        );
        if checked >= target / 2 && other.is_some() {
            let retired = Arc::downgrade(&other.as_ref().unwrap().native);
            drop(other.take());
            while retired.upgrade().is_some() {
                worker_retirement::WORKERS.reclaim();
                assert!(Instant::now() < deadline, "soak client did not drain");
                thread::park_timeout(Duration::from_millis(10));
            }
            detached_at = Some(checked);
        }
        if now.duration_since(last_report) >= Duration::from_mins(1) {
            eprintln!(
                "installed_pcm_soak elapsed_seconds={} checked={checked} target={target} detached_at={detached_at:?} faults=0",
                started.elapsed().as_secs()
            );
            last_report = now;
        }
        thread::park_timeout(Duration::from_millis(10));
    }
    assert!(
        started.elapsed() >= Duration::from_secs(seconds - 2),
        "native frame clock ran faster than elapsed time"
    );
    assert_no_client_overflows(&input, &output);
    drop(input);
    drop(output);
    assert_eq!(statistics.faults.load(Ordering::Acquire), 0);
    assert_eq!(
        selection(&snapshot()).channels,
        output_spec.channels,
        "native format changed during soak"
    );
    eprintln!(
        "installed_pcm_soak PASS elapsed_seconds={} checked={} channels={} rate={RATE} detached_at={detached_at:?} all_lanes_bit_exact=true faults=0",
        started.elapsed().as_secs(),
        statistics.checked.load(Ordering::Acquire),
        output_spec.channels
    );
}

fn assert_no_client_overflows(input: &Client, output: &Client) {
    for client in [input, output] {
        // SAFETY: connected Client retains the native owner; query is atomic only.
        assert_eq!(
            unsafe { ffi::mr_device_io_overflows(client.native.pointer.as_ptr()) },
            0
        );
    }
}

fn assert_no_pcm_faults(statistics: &Statistics) {
    assert_eq!(
        statistics.faults.load(Ordering::Acquire),
        0,
        "soak PCM fault: frame={} lane={} expected={:#x} actual={:#x}",
        statistics.bad_frame.load(Ordering::Relaxed),
        statistics.bad_lane.load(Ordering::Relaxed),
        statistics.expected.load(Ordering::Relaxed),
        statistics.actual.load(Ordering::Relaxed)
    );
}

#[test]
fn looping_words_validate_wrap_and_reject_a_gap_or_repeat_at_the_boundary() {
    let samples: Vec<_> = (FRAME_LIMIT - 3..FRAME_LIMIT + 3)
        .flat_map(|frame| (0..2).map(move |lane| marker(frame, lane)))
        .collect();
    let mut sequence = Sequence {
        looping: true,
        ..Sequence::default()
    };
    assert_eq!(sequence.check(&samples, 2, 0), Ok(6));
    assert_eq!(sequence.next, Some(3));
    for defect in [0, 1] {
        let mut changed = samples.clone();
        if defect == 0 {
            changed.drain(6..8);
        } else {
            changed.splice(6..6, samples[4..6].iter().copied());
        }
        assert!(
            Sequence {
                looping: true,
                ..Sequence::default()
            }
            .check(&changed, 2, 0)
            .is_err()
        );
    }
}

#[cfg(feature = "native-lab")]
fn selection(snapshot: &mixrelay_driver_core::DriverSnapshot) -> Spec {
    let uid = std::env::var("MIXRELAY_TEST_DRIVER_UID")
        .expect("explicit canonical installed Driver UID required");
    let channels: u16 = std::env::var("MIXRELAY_TEST_DRIVER_CHANNELS")
        .expect("explicit expected native width required")
        .parse()
        .expect("native channel count");
    assert!(matches!(channels, 2 | 32 | 64));
    let sample_format = match std::env::var("MIXRELAY_TEST_DRIVER_FORMAT").as_deref() {
        Ok("float32") => NativeSampleFormat::Float32,
        Ok("signed16") => NativeSampleFormat::Signed16,
        Ok("signed24") => NativeSampleFormat::Signed24,
        Ok("signed32") => NativeSampleFormat::Signed32,
        _ => panic!("explicit native float32/signed16/signed24/signed32 required"),
    };
    let device = snapshot
        .settings
        .devices
        .iter()
        .find(|device| device.native_device_ids.contains(&uid))
        .expect("exact installed UID absent");
    let id =
        mixrelay_macos_transport::endpoint_identity::parse_endpoint_id(device.spec.id.as_str())
            .expect("canonical Driver device identity");
    assert_eq!(
        uid,
        mixrelay_macos_transport::endpoint_identity::endpoint_uid(id)
    );
    let expected = Some(NativeStreamFormat {
        sample_rate: RATE,
        channels,
        sample_format,
    });
    assert_eq!(device.observed_sample_rate, Some(RATE));
    assert_eq!(
        device.native_formats.render.current, expected,
        "actual playback ASBD"
    );
    assert_eq!(
        device.native_formats.capture.current, expected,
        "actual capture ASBD"
    );
    Spec {
        selector: DeviceSelector::PersistentUid { uid },
        input: false,
        channels,
    }
}

/// One duplex device cycle: the output client's presented PCM is what the input client
/// captures in the same cycle, as the Driver's audio return path presents and captures one timeline.
/// Each callback runs through the simulated HAL's started registration, so a client whose
/// registration was lost or stopped fails the cycle instead of receiving PCM.
#[cfg(test)]
struct Loopback {
    host: Arc<SimulatedHost>,
    pcm: Vec<f32>,
}

#[cfg(test)]
impl Loopback {
    const FRAMES: u32 = DEFAULT_BUFFER;

    fn new(host: Arc<SimulatedHost>, channels: u16) -> Self {
        Self {
            host,
            pcm: vec![0.0; Self::FRAMES as usize * usize::from(channels)],
        }
    }

    fn cycle(&mut self, output: &Client, input: &Client) {
        assert_no_pcm_faults(&output.probe.statistics);
        for (client, role) in [(output, "output"), (input, "input")] {
            assert!(
                self.host.callback_on(&client.native, &mut self.pcm),
                "the {role} client's registration is no longer started"
            );
        }
    }

    /// Cycles until `target` frames were checked. Once primed, every cycle checks one whole
    /// buffer, so exceeding that cycle budget means PCM was lost or stalled.
    fn run_until(&mut self, statistics: &Statistics, target: u64, clients: (&Client, &Client)) {
        let start = statistics.checked.load(Ordering::Acquire);
        let budget =
            (target.saturating_sub(start) + u64::from(PRIMING)) / u64::from(Self::FRAMES) + 2;
        for _ in 0..budget {
            if statistics.checked.load(Ordering::Acquire) >= target {
                return;
            }
            self.cycle(clients.0, clients.1);
        }
        let checked = statistics.checked.load(Ordering::Acquire);
        assert!(
            checked >= target,
            "PCM progress stalled: checked={checked} target={target} cycles={budget}"
        );
    }
}

#[test]
fn pcm_continuity_survives_other_client_detach() {
    let host = SimulatedHost::new();
    let uid = host.plug("io.mixrelay.test.loopback", 2, RATE, CAPTURED);
    let output_spec = Spec {
        selector: DeviceSelector::PersistentUid { uid },
        input: false,
        channels: 2,
    };
    let input_spec = Spec {
        input: true,
        ..output_spec.clone()
    };
    let statistics = Arc::new(Statistics::default());
    let output = Client::connect(&output_spec, &statistics, host.clone());
    let input = Client::connect(&input_spec, &statistics, host.clone());
    let mut loopback = Loopback::new(host.clone(), output_spec.channels);
    // This additional client uses the ordinary graph Device/worker lifecycle.
    let other = Device::open(&input_spec, RATE, host).expect("ordinary graph capture client");
    let retired = Arc::downgrade(&other.native);
    loopback.run_until(&statistics, PHASE_FRAMES, (&output, &input));
    drop(other);
    // PCM keeps flowing while the other client's worker unregisters and retires.
    for _ in 0..DETACH_CYCLES {
        loopback.cycle(&output, &input);
    }
    let wait_started = Instant::now();
    while {
        worker_retirement::WORKERS.reclaim();
        retired.upgrade().is_some()
    } {
        assert!(
            wait_started.elapsed() < Duration::from_secs(20),
            "native device transition stalled"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let detached_at = statistics.checked.load(Ordering::Acquire);
    loopback.run_until(&statistics, detached_at + PHASE_FRAMES, (&output, &input));
    assert_no_client_overflows(&input, &output);
    drop(input);
    drop(output);
    assert_no_pcm_faults(&statistics);
    let checked = statistics.checked.load(Ordering::Acquire);
    assert!(checked >= PHASE_FRAMES * 2);
    eprintln!(
        "simulated duplex PCM: channels={} rate={RATE} fresh_frames={checked} detached_at={detached_at} post_detach_frames={} priming={PRIMING} exact_words=true faults=0",
        output_spec.channels,
        checked - detached_at
    );
}

#[test]
fn words_survive_native_codecs_and_reject_lost_repeated_or_wrong_lanes() {
    use mixrelay_pcm_ring::SampleFormat;
    for channels in [2_u16, 32, 64] {
        let original: Vec<_> = (960..1100)
            .flat_map(|frame| (0..channels).map(move |lane| marker(frame, lane)))
            .collect();
        for format in [
            SampleFormat::Float32,
            SampleFormat::Signed16,
            SampleFormat::Signed24,
            SampleFormat::Signed32,
        ] {
            let mut bytes = vec![0; original.len() * format.bytes_per_sample()];
            let mut restored = vec![0.0; original.len()];
            assert!(format.encode(&original, &mut bytes));
            assert!(format.decode(&bytes, &mut restored));
            assert_eq!(Sequence::default().check(&restored, channels, 0), Ok(140));
        }
        for defect in 0..3 {
            let mut changed = original.clone();
            let at = 40 * usize::from(channels);
            let width = usize::from(channels);
            match defect {
                0 => {
                    changed.drain(at..at + width);
                }
                1 => {
                    changed.splice(at..at, original[at - width..at].iter().copied());
                }
                _ => changed.swap(at, at + 1),
            }
            assert!(Sequence::default().check(&changed, channels, 0).is_err());
        }
    }
    let mut sequence = Sequence::default();
    assert_eq!(sequence.check(&[0.0; 4], 2, 2), Ok(0));
    assert!(sequence.check(&[0.0; 2], 2, 2).is_err());
}
