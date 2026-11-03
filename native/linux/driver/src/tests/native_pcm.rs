#![cfg_attr(
    feature = "native-lab",
    expect(
        clippy::unwrap_used,
        reason = "Opt-in native fixture helpers fail the test immediately when preparation fails."
    )
)]
//! Native stream lifecycle fixtures and allocation-free indexed PCM witnesses.

#[cfg(feature = "native-lab")]
use crate::LinuxDriver;
#[cfg(feature = "native-lab")]
use crate::native_names;
#[cfg(feature = "native-lab")]
use crate::tests::config;
#[cfg(feature = "native-lab")]
use mixrelay_audio_io::NativeSampleFormat;
#[cfg(feature = "native-lab")]
use mixrelay_audio_io::NativeStreamFormat;
#[cfg(feature = "native-lab")]
use mixrelay_driver_core::{DriverBackend, DriverState};
#[cfg(feature = "native-lab")]
use mixrelay_linux_pipewire::{
    DriverCycle, DriverError, DriverFormat, DriverProcess, GraphDriver, GraphInputs, NativeStream,
    StreamFormat, StreamOwners, StreamRole, TransferSnapshot,
};
#[cfg(feature = "native-lab")]
use std::sync::{Arc, atomic::AtomicU32};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[cfg(feature = "native-lab")]
struct Writer {
    stream: NativeStream,
    samples: Box<[f32]>,
    channels: usize,
}

// Two 10-bit words outlast fifteen seconds at 48 kHz without wrapping. Every
// channel occupies a distinct PCM16 band and changes with the low word. These
// binary fractions also survive the production PCM24/32 and Float32 codecs.
const FRAME_LIMIT: u32 = 1 << 20;
#[cfg(feature = "native-lab")]
const PCM_FRAMES: u64 = 480_000;

fn sample_word(word: u16, lane: u16) -> f32 {
    f32::from(lane) / 32.0 - 1.0 + f32::from(word) / 32768.0
}

fn indexed_sample(sequence: u32, lane: u16) -> f32 {
    let [a, b, c, _] = sequence.to_le_bytes();
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

fn run_sample(sample: f32, inverted: bool) -> f32 {
    // Reflect signed PCM16 integers q to -q-1 so both endpoints remain exact.
    if inverted {
        -sample - 1.0 / 32768.0
    } else {
        sample
    }
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

fn indexed_frame_matches(frame: &[f32], sequence: u32, inverted: bool) -> bool {
    // Beyond the two encoded words, expected PCM aliases an earlier sequence.
    sequence < FRAME_LIMIT
        && frame.iter().zip(0_u16..).all(|(sample, lane)| {
            sample.to_bits() == run_sample(indexed_sample(sequence, lane), inverted).to_bits()
        })
}

fn indexed_sequence(frame: &[f32], inverted: bool) -> Option<u32> {
    let low = decode_word(run_sample(frame[0], inverted), 0)?;
    let mixed = decode_word(run_sample(frame[1], inverted), 1)?;
    let high = mixed.wrapping_sub(low.wrapping_mul(17)) & 1023;
    let sequence = u32::from(low) | (u32::from(high) << 10);
    indexed_frame_matches(frame, sequence, inverted).then_some(sequence)
}

#[cfg(feature = "native-lab")]
struct IndexedWriter {
    stream: NativeStream,
    samples: Box<[f32]>,
    channels: usize,
    sequence: u32,
    inverted: bool,
}

#[cfg(feature = "native-lab")]
fn print_stream_snapshot(role: &str, stream: &NativeStream) {
    let mut snapshot = TransferSnapshot::default();
    snapshot.include(stream);
    eprintln!("native_driver_pcm owner={role} snapshot={snapshot:?}");
}

#[cfg(feature = "native-lab")]
impl Drop for IndexedWriter {
    fn drop(&mut self) {
        // GraphDriver has drained its native callback before releasing this processor.
        eprintln!(
            "native_driver_pcm produced_indexed_frames={}",
            self.sequence
        );
        print_stream_snapshot("ordinary-writer", &self.stream);
    }
}

#[cfg(feature = "native-lab")]
impl DriverProcess for IndexedWriter {
    fn process(&mut self, cycle: &DriverCycle) {
        let count = cycle.frames as usize * self.channels;
        let Some(samples) = self.samples.get_mut(..count) else {
            return;
        };
        if self.sequence + cycle.frames > FRAME_LIMIT {
            return;
        }
        for frame in samples.chunks_exact_mut(self.channels) {
            for (sample, lane) in frame.iter_mut().zip(0_u16..) {
                *sample = run_sample(indexed_sample(self.sequence, lane), self.inverted);
            }
            self.sequence += 1;
        }
        self.stream.write(samples);
        self.stream.trigger_output();
    }
}

#[derive(Default)]
struct Continuity {
    fresh: AtomicU64,
    // Sum of distances from the frozen expected index; not a lost-frame count.
    missing: AtomicU64,
    repeated: AtomicU64,
    wrong: AtomicU64,
    short: AtomicU64,
    first_sequence_fault: AtomicU64,
    first_missing: AtomicU64,
}

impl Continuity {
    fn record_sequence_fault(&self, expected: u32, actual: u32) {
        // The serialized reader callback is the only writer. Locked sequences
        // expect at least frame one, so a zero packed expectation means unset.
        if self.first_sequence_fault.load(Ordering::Relaxed) == 0 {
            self.first_sequence_fault.store(
                (u64::from(expected) << 32) | u64::from(actual),
                Ordering::Relaxed,
            );
        }
        if actual > expected && self.first_missing.load(Ordering::Relaxed) == 0 {
            self.first_missing
                .store(u64::from(actual - expected), Ordering::Relaxed);
        }
    }
}

#[cfg(feature = "native-lab")]
struct IndexedReader {
    stream: NativeStream,
    samples: Box<[f32]>,
    channels: usize,
    witness: SequenceWitness,
    counters: Arc<Continuity>,
}

#[cfg(feature = "native-lab")]
impl Drop for IndexedReader {
    fn drop(&mut self) {
        // GraphDriver destruction has already drained the capture processor callback.
        print_stream_snapshot("ordinary-reader", &self.stream);
    }
}

#[derive(Default)]
struct SequenceWitness {
    next: Option<u32>,
    locked: bool,
    inverted: bool,
}

impl SequenceWitness {
    fn observe(&mut self, frame: &[f32], counters: &Continuity) {
        if self.locked
            && let Some(next) = self.next
            && indexed_frame_matches(frame, next, self.inverted)
        {
            self.next = Some(next + 1);
            counters.fresh.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let Some(sequence) = indexed_sequence(frame, self.inverted) else {
            if self.locked {
                counters.wrong.fetch_add(1, Ordering::Relaxed);
            } else {
                self.next = None;
            }
            return;
        };
        let Some(next) = self.next else {
            self.next = Some(sequence + 1);
            return;
        };
        if !self.locked && sequence != next {
            self.next = Some(sequence + 1);
            return;
        }
        self.locked = true;
        if sequence < next {
            counters.record_sequence_fault(next, sequence);
            counters.repeated.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if sequence > next {
            counters.record_sequence_fault(next, sequence);
            counters
                .missing
                .fetch_add(u64::from(sequence - next), Ordering::Relaxed);
            return;
        }
        self.next = Some(next + 1);
        counters.fresh.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(feature = "native-lab")]
impl DriverProcess for IndexedReader {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        self.stream.bind_inputs(inputs).map_err(DriverError::Input)
    }

    fn process(&mut self, cycle: &DriverCycle) {
        let count = cycle.frames as usize * self.channels;
        if count > self.samples.len() {
            return;
        }
        let delivered = self.stream.read(&mut self.samples[..count]);
        if self.witness.locked {
            self.counters
                .short
                .fetch_add(u64::from(cycle.frames - delivered), Ordering::Relaxed);
        }
        for frame in self.samples[..count].chunks_exact(self.channels) {
            self.witness.observe(frame, &self.counters);
        }
    }
}

#[cfg(feature = "native-lab")]
fn pattern_writer(stream: NativeStream, channels: u16, count: usize) -> Writer {
    Writer {
        stream,
        samples: (0..count)
            .map(|index| {
                f32::from(u16::try_from(index % usize::from(channels) + 1).unwrap()) / 256.0
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        channels: usize::from(channels),
    }
}

#[cfg(feature = "native-lab")]
impl DriverProcess for Writer {
    fn process(&mut self, cycle: &DriverCycle) {
        let count = cycle.frames as usize * self.channels;
        if let Some(samples) = self.samples.get(..count) {
            self.stream.write(samples);
            self.stream.trigger_output();
        }
    }
}

#[cfg(feature = "native-lab")]
struct Reader {
    stream: NativeStream,
    samples: Box<[f32]>,
    expected: Box<[f32]>,
    channels: usize,
    fresh: Arc<AtomicU64>,
    wrong: Arc<AtomicU64>,
}

#[cfg(feature = "native-lab")]
struct Feedback {
    reader: Reader,
    contribution: NativeStream,
    last_sample: Arc<AtomicU32>,
}

#[cfg(feature = "native-lab")]
impl DriverProcess for Feedback {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        self.reader.prepare_inputs(inputs)
    }

    fn process(&mut self, cycle: &DriverCycle) {
        self.reader.process(cycle);
        self.last_sample
            .store(self.reader.samples[0].to_bits(), Ordering::Relaxed);
        let count = cycle.frames as usize * self.reader.channels;
        let Some(samples) = self.reader.samples.get_mut(..count) else {
            return;
        };
        for sample in &mut *samples {
            *sample *= 0.5;
        }
        self.contribution.write(samples);
        self.contribution.trigger_output();
    }
}

#[cfg(feature = "native-lab")]
impl DriverProcess for Reader {
    fn prepare_inputs(&mut self, inputs: &GraphInputs) -> Result<(), DriverError> {
        self.stream.bind_inputs(inputs).map_err(DriverError::Input)
    }

    fn process(&mut self, cycle: &DriverCycle) {
        let count = cycle.frames as usize * self.channels;
        let Some(samples) = self.samples.get_mut(..count) else {
            return;
        };
        if self.stream.read(samples) != cycle.frames {
            return;
        }
        if samples.chunks_exact(self.channels).all(|frame| {
            frame
                .iter()
                .zip(&self.expected)
                .all(|(sample, expected)| sample.to_bits() == expected.to_bits())
        }) {
            self.fresh
                .fetch_add(u64::from(cycle.frames), Ordering::Relaxed);
        } else if self.fresh.load(Ordering::Relaxed) != 0 {
            self.wrong
                .fetch_add(u64::from(cycle.frames), Ordering::Relaxed);
        }
    }
}

#[cfg(feature = "native-lab")]
fn check_continuity(counters: &Continuity) {
    eprintln!(
        "native_driver_pcm fresh={} missing_gap_distance_sum={} repeated={} wrong={} short={}",
        counters.fresh.load(Ordering::Acquire),
        counters.missing.load(Ordering::Acquire),
        counters.repeated.load(Ordering::Acquire),
        counters.wrong.load(Ordering::Acquire),
        counters.short.load(Ordering::Acquire),
    );
    let fault = counters.first_sequence_fault.load(Ordering::Acquire);
    if fault != 0 {
        eprintln!(
            "native_driver_pcm first_sequence_expected={} actual={} first_gap_frames={}",
            fault >> 32,
            fault & u64::from(u32::MAX),
            counters.first_missing.load(Ordering::Acquire),
        );
    }
    assert!(
        counters.fresh.load(Ordering::Acquire) >= PCM_FRAMES,
        "native loopback did not deliver 480,000 indexed frames"
    );
    assert_eq!(
        counters.missing.load(Ordering::Acquire),
        0,
        "native PCM skipped sequence frames"
    );
    assert_eq!(
        counters.repeated.load(Ordering::Acquire),
        0,
        "native PCM repeated sequence frames"
    );
    assert_eq!(
        counters.wrong.load(Ordering::Acquire),
        0,
        "native PCM had wrong lanes or malformed/silent frames"
    );
    assert_eq!(
        counters.short.load(Ordering::Acquire),
        0,
        "native capture read was short after startup lock"
    );
}

#[cfg(feature = "native-lab")]
fn observe_pcm(channels: u16, names: &[String], inverted: bool) {
    let owners = StreamOwners::default();
    let format = StreamFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let writer = NativeStream::open_format(
        &owners,
        "mixrelay.driver-fixture.writer",
        "Ordinary playback client",
        StreamRole::DeviceOutput(&names[0]),
        channels,
        format,
    )
    .unwrap();
    let reader = NativeStream::open_format(
        &owners,
        "mixrelay.driver-fixture.reader",
        "Ordinary capture client",
        StreamRole::DeviceInput(&names[1]),
        channels,
        format,
    )
    .unwrap();
    let count = 8192 * usize::from(channels);
    let counters = Arc::new(Continuity::default());
    let writer = GraphDriver::open(
        IndexedWriter {
            stream: writer,
            samples: vec![0.0; count].into_boxed_slice(),
            channels: usize::from(channels),
            sequence: 0,
            inverted,
        },
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    let reader = GraphDriver::open(
        IndexedReader {
            stream: reader,
            samples: vec![0.0; count].into_boxed_slice(),
            channels: usize::from(channels),
            witness: SequenceWitness {
                inverted,
                ..SequenceWitness::default()
            },
            counters: Arc::clone(&counters),
        },
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while counters.fresh.load(Ordering::Acquire) < PCM_FRAMES && Instant::now() < deadline {
        // Control-side observation only; producer and consumer are native callbacks.
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(reader);
    drop(writer);
    check_continuity(&counters);
}

#[test]
fn indexed_pcm_witness_detects_skips_repeats_and_lane_corruption_without_allocation() {
    let counters = Continuity::default();
    let frame = |sequence, inverted| {
        std::array::from_fn::<_, 64, _>(|lane| {
            run_sample(
                indexed_sample(sequence, u16::try_from(lane).unwrap()),
                inverted,
            )
        })
    };
    let measured = allocation_counter::measure(|| {
        for defect in 0..3 {
            let mut witness = SequenceWitness::default();
            witness.observe(&[0.0; 64], &counters);
            witness.observe(&frame(100, false), &counters);
            witness.observe(&frame(101, false), &counters);
            let mut changed = match defect {
                0 => frame(104, false), // Exactly 102 and 103 are missing.
                1 => frame(101, false),
                _ => frame(102, false),
            };
            if defect == 2 {
                changed[63] = changed[62];
            }
            witness.observe(&changed, &counters);
            assert_eq!(witness.next, Some(102), "a fault must not resynchronize");
        }
    });
    assert_eq!(counters.fresh.load(Ordering::Relaxed), 3);
    assert_eq!(counters.missing.load(Ordering::Relaxed), 2);
    assert_eq!(counters.repeated.load(Ordering::Relaxed), 1);
    assert_eq!(counters.wrong.load(Ordering::Relaxed), 1);
    let fault = counters.first_sequence_fault.load(Ordering::Relaxed);
    assert_eq!(fault >> 32, 102);
    assert_eq!(fault & u64::from(u32::MAX), 104);
    assert_eq!(counters.first_missing.load(Ordering::Relaxed), 2);
    assert_eq!(measured.count_total, 0);
}

#[test]
fn indexed_pcm_witness_accepts_codecs_and_rejects_sequence_wrap_without_allocation() {
    use mixrelay_pcm_ring::SampleFormat;

    let frame = |sequence, inverted| {
        std::array::from_fn::<_, 64, _>(|lane| {
            run_sample(
                indexed_sample(sequence, u16::try_from(lane).unwrap()),
                inverted,
            )
        })
    };
    let measured = allocation_counter::measure(|| {
        for (channels, inverted) in [
            (2, false),
            (2, true),
            (32, false),
            (32, true),
            (64, false),
            (64, true),
        ] {
            for format in [
                SampleFormat::Float32,
                SampleFormat::Signed16,
                SampleFormat::Signed24,
                SampleFormat::Signed32,
            ] {
                let mut bytes = [0; 64 * 4];
                let bytes = &mut bytes[..channels * format.bytes_per_sample()];
                let mut restored = [0.0; 64];
                for sequence in [0, 1, 1023, 1024, 480_000, FRAME_LIMIT - 1] {
                    let original = frame(sequence, inverted);
                    assert!(format.encode(&original[..channels], bytes));
                    assert!(format.decode(bytes, &mut restored[..channels]));
                    assert_eq!(
                        indexed_sequence(&restored[..channels], inverted),
                        Some(sequence)
                    );
                    assert!(
                        original[..channels]
                            .iter()
                            .zip(&restored[..channels])
                            .all(|(original, restored)| original.to_bits() == restored.to_bits())
                    );
                    let counts = Continuity::default();
                    let mut witness = SequenceWitness {
                        next: Some(sequence),
                        locked: true,
                        inverted,
                    };
                    witness.observe(&restored[..channels], &counts);
                    assert_eq!(witness.next, Some(sequence + 1));
                    assert_eq!(counts.fresh.load(Ordering::Relaxed), 1);
                    assert_eq!(counts.wrong.load(Ordering::Relaxed), 0);
                    if sequence == FRAME_LIMIT - 1 {
                        witness.observe(&frame(0, inverted)[..channels], &counts);
                        assert_eq!(witness.next, Some(FRAME_LIMIT));
                        assert_eq!(counts.fresh.load(Ordering::Relaxed), 1);
                        assert_eq!(counts.repeated.load(Ordering::Relaxed), 1);
                    }
                }
            }
        }
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
fn indexed_pcm_witness_checks_every_lane_without_allocation() {
    let frame = |sequence, inverted| {
        std::array::from_fn::<_, 64, _>(|lane| {
            run_sample(
                indexed_sample(sequence, u16::try_from(lane).unwrap()),
                inverted,
            )
        })
    };
    let measured = allocation_counter::measure(|| {
        for (channels, inverted) in [
            (2, false),
            (2, true),
            (32, false),
            (32, true),
            (64, false),
            (64, true),
        ] {
            for lane in 0..channels {
                let counts = Continuity::default();
                let mut witness = SequenceWitness {
                    next: Some(10),
                    locked: true,
                    inverted,
                };
                let mut corrupted = frame(10, inverted);
                corrupted[lane] = f32::NAN;
                witness.observe(&corrupted[..channels], &counts);
                assert_eq!(witness.next, Some(10));
                assert_eq!(counts.fresh.load(Ordering::Relaxed), 0);
                assert_eq!(counts.wrong.load(Ordering::Relaxed), 1);
                witness.observe(&frame(10, inverted)[..channels], &counts);
                assert_eq!(witness.next, Some(11));
                assert_eq!(counts.fresh.load(Ordering::Relaxed), 1);
            }
        }
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
#[ignore = "bounded release timing; run serially with no concurrent build or benchmark"]
fn sequence_witness_release_timing() {
    use std::hint::black_box;

    const FRAMES: u32 = 128;
    const ITERATIONS: u32 = 1000;
    assert!(
        black_box(!cfg!(debug_assertions)),
        "run this measurement with --release"
    );
    for channels in [32_usize, 64] {
        let samples: Vec<f32> = (0..FRAMES)
            .flat_map(|sequence| {
                (0..channels)
                    .map(move |lane| indexed_sample(sequence, u16::try_from(lane).unwrap()))
            })
            .collect();
        let mut elapsed = [Duration::ZERO; 7];
        for run in 0..8 {
            let counters = Continuity::default();
            let mut witness = SequenceWitness {
                next: Some(0),
                locked: true,
                inverted: false,
            };
            let started = Instant::now();
            for _ in 0..ITERATIONS {
                witness.next = Some(0);
                for frame in black_box(&samples).chunks_exact(channels) {
                    witness.observe(black_box(frame), black_box(&counters));
                }
                black_box(&witness);
            }
            let duration = started.elapsed();
            assert_eq!(witness.next, Some(FRAMES));
            assert!(witness.locked);
            assert_eq!(
                counters.fresh.load(Ordering::Relaxed),
                u64::from(FRAMES) * u64::from(ITERATIONS)
            );
            assert_eq!(counters.missing.load(Ordering::Relaxed), 0);
            assert_eq!(counters.repeated.load(Ordering::Relaxed), 0);
            assert_eq!(counters.wrong.load(Ordering::Relaxed), 0);
            assert_eq!(counters.short.load(Ordering::Relaxed), 0);
            assert_eq!(counters.first_sequence_fault.load(Ordering::Relaxed), 0);
            assert_eq!(counters.first_missing.load(Ordering::Relaxed), 0);
            assert!(samples.iter().enumerate().all(|(index, sample)| {
                sample.to_bits()
                    == indexed_sample(
                        u32::try_from(index / channels).unwrap(),
                        u16::try_from(index % channels).unwrap(),
                    )
                    .to_bits()
            }));
            if run != 0 {
                elapsed[run - 1] = duration;
            }
        }
        elapsed.sort_unstable();
        eprintln!(
            "sequence_witness channels={channels} frames={FRAMES} calls={ITERATIONS} min_ns_per_128block={:.3} median_ns_per_128block={:.3} max_ns_per_128block={:.3}",
            elapsed[0].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
            elapsed[3].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
            elapsed[6].as_secs_f64() * 1_000_000_000.0 / f64::from(ITERATIONS),
        );
    }
}

#[test]
fn indexed_pcm_witness_rejects_previous_scope_without_allocation() {
    let frame = |sequence, inverted| {
        std::array::from_fn::<_, 64, _>(|lane| {
            run_sample(
                indexed_sample(sequence, u16::try_from(lane).unwrap()),
                inverted,
            )
        })
    };
    let measured = allocation_counter::measure(|| {
        for channels in [2, 32, 64] {
            for inverted in [false, true] {
                let counters = Continuity::default();
                let mut witness = SequenceWitness {
                    inverted,
                    ..SequenceWitness::default()
                };
                witness.observe(&frame(483_326, !inverted)[..channels], &counters);
                witness.observe(&frame(483_327, !inverted)[..channels], &counters);
                assert!(!witness.locked);
                assert_eq!(witness.next, None);
                assert_eq!(counters.fresh.load(Ordering::Relaxed), 0);
                assert_eq!(counters.wrong.load(Ordering::Relaxed), 0);
                witness.observe(&frame(0, inverted)[..channels], &counters);
                witness.observe(&frame(1, inverted)[..channels], &counters);
                assert!(witness.locked);
                assert_eq!(witness.next, Some(2));
                witness.observe(&frame(483_328, !inverted)[..channels], &counters);
                assert_eq!(counters.wrong.load(Ordering::Relaxed), 1);
                assert_eq!(witness.next, Some(2));
                witness.observe(&frame(2, inverted)[..channels], &counters);
                witness.observe(&frame(5, inverted)[..channels], &counters);
                witness.observe(&frame(2, inverted)[..channels], &counters);
                let mut corrupted = frame(3, inverted);
                corrupted.swap(0, 1);
                witness.observe(&corrupted[..channels], &counters);
                assert_eq!(witness.next, Some(3), "a fault must not resynchronize");
                assert_eq!(counters.fresh.load(Ordering::Relaxed), 2);
                assert_eq!(counters.missing.load(Ordering::Relaxed), 2);
                assert_eq!(counters.repeated.load(Ordering::Relaxed), 1);
                assert_eq!(counters.wrong.load(Ordering::Relaxed), 2);
            }
        }
    });
    assert_eq!(measured.count_total, 0);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_driver_reference_loopback_continuity() {
    use std::process::{Command, Stdio};

    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let names = native_names(&config(32).spec);
    let positions = format!(
        "[ {} ]",
        (0..32)
            .map(|lane| format!("AUX{lane}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let capture = format!(
        "node.name={} media.class=Audio/Sink node.autoconnect=false audio.rate=48000 node.rate=1/48000 node.latency=128/48000 node.pause-on-idle=false node.always-process=true",
        names[0]
    );
    let playback = format!(
        "node.name={} media.class=Audio/Source node.autoconnect=false audio.rate=48000 node.rate=1/48000 node.latency=128/48000 node.pause-on-idle=false node.always-process=true",
        names[1]
    );
    let mut loopback = ReferenceLoopback(
        Command::new("pw-loopback")
            .args([
                "--channels",
                "32",
                "--channel-map",
                &positions,
                "--group",
                "mixrelay.graph",
                "--capture-props",
                &capture,
                "--playback-props",
                &playback,
            ])
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    loopback.wait_ready(&names);
    // PipeWire 1.4.2 native forwarding module fixes both streams to F32P. This is a
    // scheduling control with identical ordinary clients/PCM, not PCM24 coverage.
    for scope in 1..=2 {
        eprintln!("reference_loopback_pcm 32ch F32P scope={scope}/2 start");
        observe_pcm(32, &names, scope == 2);
        eprintln!("reference_loopback_pcm 32ch F32P scope={scope}/2 passed");
    }
}

#[cfg(feature = "native-lab")]
struct ReferenceLoopback(std::process::Child);

#[cfg(feature = "native-lab")]
impl ReferenceLoopback {
    fn wait_ready(&mut self, names: &[String]) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(
                self.0.try_wait().unwrap().is_none(),
                "pw-loopback exited before publishing its nodes"
            );
            let targets = mixrelay_linux_audio_io::targets().unwrap();
            if names
                .iter()
                .zip(["Audio/Sink", "Audio/Source"])
                .all(|(name, class)| {
                    targets.iter().any(|target| {
                        target.node_name.as_ref() == Some(name)
                            && target.media_class == class
                            && target.channels == 32
                    })
                })
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "reference loopback nodes did not publish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(feature = "native-lab")]
impl Drop for ReferenceLoopback {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
#[cfg(feature = "native-lab")]
fn native_driver_32_64_pcm_continues_after_graph_client_exit() {
    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let channels_filter =
        std::env::var_os("MIXRELAY_LAB_DRIVER_CHANNELS").map(|value| match value.to_str() {
            Some("32") => 32,
            Some("64") => 64,
            _ => panic!("MIXRELAY_LAB_DRIVER_CHANNELS must be exactly 32 or 64"),
        });
    let format_filter =
        std::env::var_os("MIXRELAY_LAB_DRIVER_FORMAT").map(|value| match value.to_str() {
            Some("float32") => NativeSampleFormat::Float32,
            Some("s16") => NativeSampleFormat::Signed16,
            Some("s24") => NativeSampleFormat::Signed24,
            Some("s32") => NativeSampleFormat::Signed32,
            _ => panic!("MIXRELAY_LAB_DRIVER_FORMAT must be exactly float32, s16, s24, or s32"),
        });
    let mut driver = LinuxDriver::open().unwrap();
    let mut cases = 0;
    for channels in [32, 64]
        .into_iter()
        .filter(|channels| channels_filter.is_none_or(|selected| selected == *channels))
    {
        let mut requested = config(channels);
        let names = native_names(&requested.spec);
        for sample_format in [
            NativeSampleFormat::Float32,
            NativeSampleFormat::Signed16,
            NativeSampleFormat::Signed24,
            NativeSampleFormat::Signed32,
        ]
        .into_iter()
        .filter(|format| format_filter.is_none_or(|selected| selected == *format))
        {
            let format = NativeStreamFormat {
                sample_rate: 48_000,
                channels,
                sample_format,
            };
            requested.render_format = Some(format);
            requested.capture_format = Some(format);
            driver.replace(&[requested.clone()]).unwrap();
            cases += 1;
            // Each observation ends both ordinary-client scopes. The second
            // independent pair must still receive the same native Driver PCM.
            for scope in 1..=2 {
                eprintln!("native_driver_pcm {channels}ch {sample_format:?} scope={scope}/2 start");
                check_native_settings(&driver, format, &names);
                observe_pcm(channels, &names, scope == 2);
                check_native_settings(&driver, format, &names);
                eprintln!(
                    "native_driver_pcm {channels}ch {sample_format:?} scope={scope}/2 passed"
                );
            }
        }
    }
    assert!(cases > 0, "native Driver PCM filters selected no cases");
}

#[test]
#[cfg(feature = "native-lab")]
fn native_installed_driver_pcm_survives_client_exit() {
    let render = std::env::var("MIXRELAY_LAB_DRIVER_RENDER").unwrap();
    let capture = std::env::var("MIXRELAY_LAB_DRIVER_CAPTURE").unwrap();
    let channels = match std::env::var("MIXRELAY_LAB_DRIVER_CHANNELS")
        .unwrap()
        .as_str()
    {
        "2" => 2,
        "32" => 32,
        "64" => 64,
        _ => panic!("MIXRELAY_LAB_DRIVER_CHANNELS must be exactly 2, 32, or 64"),
    };
    let root = render
        .strip_suffix(".render")
        .expect("canonical render suffix");
    let digest = root
        .strip_prefix("io.mixrelay.endpoint.")
        .expect("canonical Driver endpoint prefix");
    assert!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Driver endpoint must contain exactly 64 lowercase hex digits"
    );
    assert_eq!(capture, format!("{root}.capture"), "canonical duplex pair");
    let names = [render, capture];
    for scope in 1..=2 {
        eprintln!("installed_driver_pcm {channels}ch scope={scope}/2 start");
        observe_pcm(channels, &names, scope == 2);
        eprintln!("installed_driver_pcm {channels}ch scope={scope}/2 passed");
    }
}

#[cfg(feature = "native-lab")]
fn check_native_settings(driver: &LinuxDriver, format: NativeStreamFormat, names: &[String]) {
    assert_eq!(driver.state(), DriverState::Ready);
    let settings = driver.settings().unwrap();
    assert_eq!(settings.devices.len(), 1);
    let device = &settings.devices[0];
    assert_eq!(device.native_formats.render.current, Some(format));
    assert_eq!(device.native_formats.capture.current, Some(format));
    assert_eq!(device.native_device_ids, names);
}

#[cfg(feature = "native-lab")]
fn removal_clients(
    owners: &StreamOwners,
    names: &[String],
    fresh: &Arc<AtomicU64>,
) -> (GraphDriver<Writer>, GraphDriver<Reader>) {
    let format = StreamFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let writer = NativeStream::open_format(
        owners,
        "mixrelay.driver-fixture.removal-writer",
        "Open playback client during Driver removal",
        StreamRole::DeviceOutput(&names[0]),
        2,
        format,
    )
    .unwrap();
    let reader = NativeStream::open_format(
        owners,
        "mixrelay.driver-fixture.removal-reader",
        "Open capture client during Driver removal",
        StreamRole::DeviceInput(&names[1]),
        2,
        format,
    )
    .unwrap();
    let writer = GraphDriver::open(
        pattern_writer(writer, 2, 8192 * 2),
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    let reader = GraphDriver::open(
        Reader {
            stream: reader,
            samples: vec![0.0; 8192 * 2].into_boxed_slice(),
            expected: vec![1.0 / 256.0, 2.0 / 256.0].into_boxed_slice(),
            channels: 2,
            fresh: Arc::clone(fresh),
            wrong: Arc::new(AtomicU64::new(0)),
        },
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    (writer, reader)
}

#[test]
#[cfg(feature = "native-lab")]
fn native_driver_removal_drains_with_playback_and_capture_clients_open() {
    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let owners = StreamOwners::default();
    assert_eq!(owners.active_owners(), 0);
    let resources = || {
        (
            std::fs::read_dir("/proc/self/fd").unwrap().count(),
            std::fs::read_dir("/proc/self/task").unwrap().count(),
        )
    };
    let requested = config(2);
    let names = native_names(&requested.spec);
    let mut baseline = None;
    for drop_service in [false, true, false, true] {
        let mut driver = LinuxDriver::open_with_owners(owners.clone()).unwrap();
        driver.replace(std::slice::from_ref(&requested)).unwrap();
        let fresh = Arc::new(AtomicU64::new(0));
        let clients = removal_clients(&owners, &names, &fresh);
        let deadline = Instant::now() + Duration::from_secs(5);
        while fresh.load(Ordering::Acquire) < 4096 {
            assert!(
                Instant::now() < deadline,
                "ordinary duplex PCM never started"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(owners.active_owners(), 4);
        if drop_service {
            drop(driver);
        } else {
            driver.replace(&[]).unwrap();
            assert!(driver.settings().unwrap().devices.is_empty());
            drop(driver);
        }
        // Client owners remain alive while both Driver acquisitions and their
        // callbacks are reclaimed.
        assert_eq!(owners.active_owners(), 2);
        std::thread::sleep(Duration::from_millis(100));
        let stopped = fresh.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            fresh.load(Ordering::Acquire),
            stopped,
            "removed device replayed PCM"
        );
        let mut replacement = LinuxDriver::open_with_owners(owners.clone()).unwrap();
        replacement
            .replace(std::slice::from_ref(&requested))
            .unwrap();
        drop(replacement);
        drop(clients);
        assert_eq!(owners.active_owners(), 0);
        let after = resources();
        if let Some(before) = baseline {
            assert_eq!(
                after, before,
                "FD/thread accumulation after Driver retirement"
            );
        } else {
            baseline = Some(after); // First duplex cycle warms native library caches.
        }
    }
}

#[cfg(feature = "native-lab")]
fn observe_feedback(channels: u16, names: &[String]) {
    let owners = StreamOwners::default();
    let format = StreamFormat {
        sample_rate: 48_000,
        block_frames: 128,
    };
    let count = 8192 * usize::from(channels);
    let open = |name, role| {
        NativeStream::open_format(&owners, name, name, role, channels, format).unwrap()
    };
    let reader = open(
        "mixrelay.driver-fixture.feedback-read",
        StreamRole::DeviceInput(&names[1]),
    );
    let contribution = open(
        "mixrelay.driver-fixture.feedback-write",
        StreamRole::GraphContribution(&names[0]),
    );
    let writer = open(
        "mixrelay.driver-fixture.feedback-source",
        StreamRole::DeviceOutput(&names[0]),
    );
    let fresh = Arc::new(AtomicU64::new(0));
    let wrong = Arc::new(AtomicU64::new(0));
    let last_sample = Arc::new(AtomicU32::new(0));
    let source = GraphDriver::open(
        pattern_writer(writer, channels, count),
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    let graph = GraphDriver::open(
        Feedback {
            reader: Reader {
                stream: reader,
                samples: vec![0.0; count].into_boxed_slice(),
                // Native mixer adds OS playback once: x + 0.5*y converges to 2*x.
                expected: (1..=channels)
                    .map(|channel| f32::from(channel) / 128.0)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                channels: usize::from(channels),
                fresh: Arc::clone(&fresh),
                wrong: Arc::clone(&wrong),
            },
            contribution,
            last_sample: Arc::clone(&last_sample),
        },
        DriverFormat::new(48_000, 128).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fresh.load(Ordering::Acquire) < 48_000 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let fresh_frames = fresh.load(Ordering::Acquire);
    let wrong_frames = wrong.load(Ordering::Acquire);
    let last_bits = last_sample.load(Ordering::Relaxed);
    let last_value = f32::from_bits(last_bits);
    let expected = 1.0_f32 / 128.0;
    eprintln!(
        "feedback_pcm channels={channels} fresh={fresh_frames} wrong={wrong_frames} last_sample={last_value} last_sample_bits={last_bits:08x} expected={expected}"
    );
    assert!(
        fresh_frames >= 48_000,
        "same-device feedback did not sustain expected PCM: fresh={fresh_frames} wrong={wrong_frames} last_sample={last_value} expected={expected}"
    );
    assert_eq!(
        wrong.load(Ordering::Acquire),
        0,
        "same-device feedback PCM broke after settling"
    );
    drop(graph);
    drop(source);
}

#[test]
#[cfg(feature = "native-lab")]
fn native_driver_same_device_capture_graph_render_has_causal_pcm() {
    let fixture = std::env::var("MIXRELAY_LAB_APPLICATION_FIXTURE").unwrap();
    assert!(std::path::Path::new(&fixture).is_file());
    let mut driver = LinuxDriver::open().unwrap();
    for channels in [32, 64] {
        let requested = config(channels);
        driver.replace(std::slice::from_ref(&requested)).unwrap();
        observe_feedback(channels, &native_names(&requested.spec));
        assert_eq!(driver.state(), DriverState::Ready);
    }
}
