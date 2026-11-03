//! Exercise the graph acquisition cursor with real partial native publications.
use super::*;
use mixrelay_audio_io::{
    capture_alignment::{CaptureAlignment, CaptureFrontier},
    process::{ClockDomain, ProcessCycle},
    session::ClockAuthority,
};

#[test]
fn partial_native_credit_preserves_every_audible_frame_through_acquisition_alignment() {
    let mut input = AlignedInput::new();
    let measured = allocation_counter::measure(|| input.run());
    assert_eq!(measured.count_total, 0);
    input.verify();
}

#[test]
fn deferred_graph_consumption_keeps_all_pcm_already_covered_by_native_credit() {
    for batch in [4_096, 16_384] {
        let mut input = AlignedInput::new();
        let measured = allocation_counter::measure(|| input.run_batched(batch));
        assert_eq!(measured.count_total, 0);
        input.verify();
    }
}

#[test]
fn preallocated_retention_recovers_four_seconds_and_a_full_ring_of_quiet_and_signal() {
    let capacity = mixrelay_audio_io::retention::RING_FRAMES;
    for batch in [192_000, u64::from(capacity)] {
        // Production geometry: playback-sized HAL client queues drained by the pump into
        // the retention-sized source while the graph defers its reads.
        let mut input = AlignedInput::with_capacity(
            capacity,
            mixrelay_macos_transport::client_pcm::QUEUE_FRAMES,
        );
        input.repeat_signal = true;
        let measured = allocation_counter::measure(|| input.run_batched(batch));
        assert_eq!(measured.count_total, 0);
        input.verify();
    }
}

struct AlignedInput {
    registry: SourceRegistry,
    transport: SourceTransport,
    converter: Converter,
    token: u64,
    writer: ClientPcmWriter,
    queues: Vec<ClientPcmReader>,
    mixer: ClientPcmMixer,
    alignment: CaptureAlignment,
    nanos: Arc<AtomicU64>,
    consumed: u64,
    published: u64,
    playhead: u64,
    repeat_signal: bool,
    scratch: [f32; 256],
    input: [f32; 256],
    times: [u64; 128],
    output: [f32; 256],
    captured: Vec<f32>,
}
impl AlignedInput {
    fn new() -> Self {
        Self::with_capacity(16_384, 16_384)
    }

    fn with_capacity(capacity: u32, queue: u32) -> Self {
        let nanos = Arc::new(AtomicU64::new(0));
        let registry =
            crate::endpoint_service::virtual_pcm::test_storage::registry(Box::new(Clock {
                nanos: nanos.clone(),
            }));
        let mut request = SourceRequest {
            capacity_frames: capacity,
            channels: 2,
            transport_version: VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request.source_id.iter_mut().zip(b"virtual:fixture") {
            *target = std::ffi::c_char::from_ne_bytes([*byte]);
        }
        let token = source_token("virtual:fixture");
        let mut transport = SourceTransport::create(&registry, &request).unwrap();
        let mut output = [0.0; 256];
        registry.read_audio(token, &mut output, 128, 48_000);
        assert!(registry.set_clock_master(token, true));
        let clock = transport.clock().unwrap();
        let converter = started(&mut transport, clock, (48_000, 2));
        let (writer, queues) = client_grants_with_capacity(queue);
        Self {
            registry,
            transport,
            converter,
            token,
            writer,
            queues,
            nanos,
            mixer: ClientPcmMixer::default(),
            alignment: CaptureAlignment::default(),
            consumed: 0,
            published: 0,
            playhead: 0,
            repeat_signal: false,
            scratch: [0.0; 256],
            input: [0.0; 256],
            times: [0; 128],
            output,
            captured: Vec::with_capacity((capacity as usize + 50_000) * 2),
        }
    }

    fn run(&mut self) {
        // Native callbacks publish 512 frames, with the playhead observed between
        // callbacks. Delayed scheduling also drains multiple slices in one burst.
        for (step, increment) in [21, 5, 23, 79, 128, 43, 85, 10, 118, 16, 112, 39, 89]
            .into_iter()
            .cycle()
            .take(800)
            .enumerate()
        {
            self.playhead += match step {
                160 => 512,
                320 => 1_024,
                520 => 768,
                _ => increment,
            };
            self.publish_callbacks();
            self.nanos
                .store(self.playhead * 1_000_000_000 / 48_000, Ordering::Relaxed);
            while let Some(window) = self.mixer.window(&self.queues, |_| Some(self.playhead)) {
                self.transfer(window);
                self.capture();
            }
        }
    }

    fn publish_callbacks(&mut self) {
        while self.published < self.playhead {
            let pcm: [f32; 1024] = std::array::from_fn(|index| {
                let frame = self.published + index as u64 / 2;
                let value = if self.audible(frame) {
                    codeword(frame)
                } else {
                    0.0
                };
                if index % 2 == 0 { value } else { -value }
            });
            assert!(self.writer.write(self.published, &pcm, 512));
            self.published += 512;
        }
    }

    fn run_batched(&mut self, batch: u64) {
        for (step, batch) in std::iter::once(2_048)
            .chain(std::iter::repeat_n(
                batch,
                usize::try_from(38_912_u64.div_ceil(batch)).unwrap(),
            ))
            .enumerate()
        {
            // The pump keeps draining client queues while only the graph is deferred.
            let deferred = self.playhead + batch;
            while self.playhead < deferred {
                self.playhead = (self.playhead + 2_048).min(deferred);
                self.publish_callbacks();
                self.nanos
                    .store(self.playhead * 1_000_000_000 / 48_000, Ordering::Relaxed);
                while let Some(window) = self.mixer.window(&self.queues, |_| Some(self.playhead)) {
                    self.transfer(window);
                }
            }
            let clock = self.registry.clock(self.token).unwrap();
            if step == 0 {
                // The pacing consumer attaches during quiet priming. Later publications
                // remain retained while the independently scheduled graph is delayed.
                assert_eq!(clock.native_frames, 2_048);
                self.consumed = clock.native_frames;
                continue;
            }
            assert_eq!(clock.native_frames - self.consumed, batch);
            while self.consumed < clock.native_frames {
                let first_frame = self.consumed - 2_048;
                self.capture_cycle(
                    ProcessCycle {
                        clock: ClockDomain {
                            authority: ClockAuthority::Device,
                            id: 3,
                            epoch: clock.epoch,
                        },
                        sample_rate: 48_000,
                        frames: 128,
                        first_frame,
                        monotonic_ns: None,
                        discontinuity: first_frame == 0,
                    },
                    u32::try_from(clock.native_frames - self.consumed).unwrap(),
                );
                self.consumed += 128;
            }
        }
    }

    fn transfer(&mut self, window: mixrelay_macos_transport::client_pcm::MixWindow) {
        let frames = window.frames as usize;
        ClientPcmMixer::mix(
            &self.queues,
            window,
            &mut self.input[..frames * 2],
            &mut self.scratch,
        )
        .unwrap();
        for (offset, time) in self.times[..frames].iter_mut().enumerate() {
            *time = 1_000_000_000 + (window.first + offset as u64) * 1_000_000_000 / 48_000;
        }
        self.converter
            .observe(&self.transport, self.transport.clock().unwrap())
            .unwrap();
        self.converter
            .push(
                &mut self.transport,
                &mut self.input[..frames * 2],
                &self.times[..frames],
            )
            .unwrap();
        assert!(self.converter.flush(&mut self.transport).unwrap());
        self.mixer.commit(window);
        self.transport.publish_measurements(
            window.frames,
            u32::try_from(self.converter.output_quantum()).unwrap(),
            0,
        );
    }

    fn capture(&mut self) {
        let clock = self.registry.clock(self.token).unwrap();
        while self.consumed < clock.native_frames {
            let frames = u32::try_from((clock.native_frames - self.consumed).min(128)).unwrap();
            let cycle = ProcessCycle {
                clock: ClockDomain {
                    authority: ClockAuthority::Device,
                    id: 3,
                    epoch: clock.epoch,
                },
                sample_rate: 48_000,
                frames,
                first_frame: self.consumed,
                monotonic_ns: None,
                discontinuity: self.consumed == 0,
            };
            self.capture_cycle(cycle, frames);
            self.consumed += u64::from(frames);
        }
    }

    fn capture_cycle(&mut self, cycle: ProcessCycle, available_frames: u32) {
        let frames = cycle.frames;
        let frontier = self
            .registry
            .source_time_for_credit(
                self.token,
                48_000,
                mixrelay_macos_transport::source_registry::GraphCredit {
                    frames,
                    available_frames,
                    published: None,
                },
            )
            .map(|time| CaptureFrontier::new(self.token, time));
        let period = self
            .registry
            .source_period(self.token)
            .unwrap_or(1e9 / 48_000.0);
        let output = &mut self.output[..frames as usize * 2];
        output.fill(0.0);
        if let Some(time) = self.alignment.begin(cycle, frontier, period) {
            self.registry.read_at(self.token, output, 48_000, time);
        }
        self.captured.extend_from_slice(output);
    }

    #[expect(
        clippy::float_cmp,
        reason = "same-clock PCM and binary codewords must match exactly, including consecutive frame spacing"
    )]
    fn verify(&self) {
        let actual: Vec<_> = self
            .captured
            .chunks_exact(2)
            .enumerate()
            .filter_map(|(index, frame)| {
                assert_eq!(frame[0], -frame[1]);
                (frame[0] != 0.0).then_some((index, frame[0]))
            })
            .collect();
        let expected: Vec<_> = (0..self.playhead)
            .filter(|&frame| self.audible(frame))
            .map(codeword)
            .collect();
        assert_eq!(
            actual.len(),
            expected.len(),
            "audible PCM was lost or duplicated"
        );
        for (frame, (&(_, actual), expected)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(
                actual, expected,
                "source order changed at audible frame {frame}"
            );
        }
        for adjacent in actual.windows(2) {
            if adjacent[1].1 - adjacent[0].1 == f32::EPSILON / 2.0 {
                assert_eq!(
                    adjacent[1].0,
                    adjacent[0].0 + 1,
                    "silence inserted inside continuous source PCM"
                );
            }
        }
    }

    fn audible(&self, frame: u64) -> bool {
        let frame = if self.repeat_signal {
            frame % 40_960
        } else {
            frame
        };
        (4_096..20_480).contains(&frame) || (24_576..40_960).contains(&frame)
    }
}

fn codeword(frame: u64) -> f32 {
    f32::from_bits(0x3f00_0000 + u32::try_from(frame).unwrap())
}
