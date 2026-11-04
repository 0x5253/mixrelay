//! A graph waiting for its first producer must retain the producer's initial PCM.
use super::*;
use mixrelay_macos_transport::application_source_transport::mr_application_source_transport_write_clocked;

#[test]
fn an_already_active_consumer_preserves_quiet_and_signal_published_before_its_next_poll() {
    preserve_committed_source(48_000, 8192, 0);
}

#[test]
fn a_fractional_native_rate_preserves_the_last_committed_graph_frame() {
    // Include the source FIFO's startup reserve before checking the finite audible tail.
    preserve_committed_source(44_100, 1025, 2048);
}

fn preserve_committed_source(native_rate: u32, total: u32, signal_offset: u64) {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status.clone(), 48_000, 128);
    let devices = publish(&resources, &[]);
    let registry = scheduled_registry();
    let application = ApplicationInput::at_rate(&registry, native_rate);
    assert!(registry.set_clock_master(source_token("app"), true));
    let mut processor = application_processor();
    let mut timeline = MacTimeline::new();
    let mut pacer = ApplicationPacer::default();
    pacer.await_source(source_token("app"));
    write_startup_source(&application, total, signal_offset);
    let mut capture = Capture {
        io: timeline.take_external_io(&devices),
        pcm: Vec::with_capacity(total as usize * 2),
    };
    let route = RouteClock {
        maximum_frames: 128,
        synchronized: true,
        independent_idle: false,
    };
    let measured = allocation_counter::measure(|| {
        for _ in 0..64 {
            let pace = pacer.next(
                (source_token("app"), application_progress(&registry)),
                (48_000, 128, 16_384),
            );
            if !matches!(pace, Pace::Block { .. }) {
                break;
            }
            let (frames, discontinuity) = timeline.accept((pace, 3), &route, &status).unwrap();
            capture.io = timeline.take_external_io(&devices);
            capture.io.sources = Some(&registry);
            process_native_cycle(
                &mut processor,
                &mut capture,
                WorkerCycle {
                    timeline: &timeline,
                    frames,
                    discontinuity,
                },
            )
            .unwrap();
            timeline.capture_alignment = std::mem::take(&mut capture.io.alignment);
        }
    });
    assert_eq!(measured.count_total, 0);
    assert_eq!(capture.pcm.len(), total as usize * 2);
    assert_eq!(
        pacer.next(
            (source_token("app"), application_progress(&registry)),
            (48_000, 128, 16_384)
        ),
        Pace::Wait
    );
    let expected: Vec<_> = (0..total)
        .step_by(128)
        .flat_map(|first| {
            startup_pcm(u64::from(first) + signal_offset)
                .into_iter()
                .take((total - first).min(128) as usize * 2)
        })
        .collect();
    assert_eq!(
        capture.pcm, expected,
        "startup publication was discarded before graph acquisition"
    );
}

/// A registry whose consumer scheduling clock stays at one instant.
fn scheduled_registry() -> mixrelay_macos_transport::source_registry::SourceRegistry {
    mixrelay_macos_transport::source_registry::SourceRegistry::new(
        Box::new(SchedulingClock {
            nanos: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }),
        mixrelay_realtime::Timebase::current(),
    )
}

fn write_startup_source(application: &ApplicationInput, total: u32, signal_offset: u64) {
    for first in (0..total).step_by(128) {
        let frames = (total - first).min(128);
        let pcm = startup_pcm(u64::from(first) + signal_offset);
        let times: [u64; 128] = std::array::from_fn(|offset| {
            1_000_000_000 + (u64::from(first) + offset as u64) * 1_000_000_000 / 48_000
        });
        // SAFETY: the fixture exclusively owns this producer; both spans contain 128 frames.
        unsafe {
            assert!(mr_application_source_transport_write_clocked(
                application.owner,
                pcm.as_ptr(),
                times.as_ptr(),
                frames
            ));
        }
    }
}

fn startup_pcm(first: u64) -> [f32; 256] {
    std::array::from_fn(|index| {
        let frame = first + index as u64 / 2;
        if (frame / 2048).is_multiple_of(2) {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "fixture frame IDs fit exactly within Float32"
        )]
        let value = (frame + 1) as f32 / 16384.0;
        if index % 2 == 0 { value } else { -value }
    })
}

/// One producer window committed after the pacer's credit snapshot, before the
/// graph requests its acquisition frontier, on the first awaited startup block.
#[test]
fn a_window_published_after_the_credit_snapshot_keeps_capture_on_credited_pcm() {
    let race = credit_race(true);
    let silent = race
        .pcm
        .chunks_exact(2)
        .filter(|frame| frame[0].to_bits() == 0)
        .count();
    assert_eq!(
        (race.pcm.len() / 2, silent),
        (usize::try_from(race.published).unwrap(), 0),
        "the acquisition cursor moved past the PCM its pacing credit covered"
    );
    let expected: Vec<_> = (0..race.published)
        .step_by(128)
        .flat_map(ramp_pcm)
        .collect();
    assert!(race.pcm == expected, "credited PCM was not contiguous");
    assert_eq!(race.time_reads, external_io::TimeReadMisses::default());
    assert_eq!(race.discontinuities, 0);
}

/// A cursor acquired beyond the pacing source's credited PCM, as the frontier
/// was before it followed the credit snapshot, can never be served by waiting.
#[test]
fn a_pacing_source_read_ahead_of_its_publication_reacquires_with_a_discontinuity() {
    let race = credit_race(false);
    let tail = &race.pcm[race.pcm.len() - 32 * 256..];
    let silent = tail
        .chunks_exact(2)
        .filter(|frame| frame[0].to_bits() == 0)
        .count();
    assert_eq!(
        silent, 0,
        "the pacing source stayed silent ahead of its publication"
    );
    assert_eq!(race.time_reads.reacquisitions, 1, "{:?}", race.time_reads);
    assert!(race.time_reads.ahead >= 1, "{:?}", race.time_reads);
    // Both aligned inputs, the application and the absent device, restart their history.
    assert_eq!(race.discontinuities, 2);
    // Shortage stays visible as silence, but delivered PCM never repeats or skips.
    let delivered: Vec<_> = race
        .pcm
        .chunks_exact(2)
        .map(|frame| frame[0])
        .filter(|sample| sample.to_bits() != 0)
        .collect();
    assert!(
        delivered
            .windows(2)
            .all(|pair| (pair[1] - pair[0] - 1.0 / 16384.0).abs() < 1e-9),
        "reacquisition replayed or skipped source PCM"
    );
}

struct CreditRace {
    pcm: Vec<f32>,
    published: u64,
    time_reads: external_io::TimeReadMisses,
    discontinuities: u64,
}

/// Runs an awaited startup whose first block races one producer window.
/// `credited` hands that block its pacing snapshot; otherwise the frontier
/// falls back to the newest publication, which includes the racing window.
fn credit_race(credited: bool) -> CreditRace {
    let time = Arc::new(mixrelay_time::ManualClock::default());
    let status = Arc::new(AdapterRuntimeStatus::new(adapter_profile(), time));
    let resources = MacDevices::new(status.clone(), 48_000, 128);
    let devices = publish(&resources, &[]);
    let registry = scheduled_registry();
    let application = ApplicationInput::at_rate(&registry, 48_000);
    assert!(registry.set_clock_master(source_token("app"), true));
    let mut processor = application_processor();
    let mut timeline = MacTimeline::new();
    let mut pacer = ApplicationPacer::default();
    pacer.await_source(source_token("app"));
    let route = RouteClock {
        maximum_frames: 128,
        synchronized: true,
        independent_idle: false,
    };
    let mut racing = Racing {
        capture: Capture {
            io: timeline.take_external_io(&devices),
            pcm: Vec::with_capacity(1 << 15),
        },
        application: &application,
        published: 0,
        race: false,
    };
    // The startup backlog exceeds every burst reserve, so the first credit
    // spans the whole retained publication.
    for _ in 0..8 {
        racing.publish();
    }
    let mut first = true;
    let mut time_reads = external_io::TimeReadMisses::default();
    let mut discontinuities = 0;
    let measured = allocation_counter::measure(|| {
        for step in 0..=64 {
            loop {
                let pace = pacer.next(
                    (source_token("app"), application_progress(&registry)),
                    (48_000, 128, 16_384),
                );
                if !matches!(pace, Pace::Block { .. }) {
                    break;
                }
                racing.race = std::mem::take(&mut first);
                let (frames, discontinuity) = timeline.accept((pace, 3), &route, &status).unwrap();
                racing.capture.io = timeline.take_external_io(&devices);
                racing.capture.io.sources = Some(&registry);
                if racing.race && !credited {
                    racing.capture.io.publication = None;
                }
                let report = process_native_cycle(
                    &mut processor,
                    &mut racing,
                    WorkerCycle {
                        timeline: &timeline,
                        frames,
                        discontinuity,
                    },
                )
                .unwrap()
                .unwrap();
                discontinuities += report.discontinuities;
                time_reads += racing.capture.io.time_reads;
                timeline.capture_alignment = std::mem::take(&mut racing.capture.io.alignment);
            }
            if step == 64 {
                break;
            }
            racing.publish();
            if step % 20 == 19 {
                // A delayed graph wake observes two producer windows at once.
                racing.publish();
            }
        }
    });
    assert_eq!(measured.count_total, 0);
    CreditRace {
        pcm: racing.capture.pcm,
        published: racing.published,
        time_reads,
        discontinuities,
    }
}

/// Commits one producer window inside `begin_cycle` when armed.
struct Racing<'a, 'b> {
    capture: Capture<'a>,
    application: &'b ApplicationInput,
    published: u64,
    race: bool,
}
impl Racing<'_, '_> {
    fn publish(&mut self) {
        let pcm = ramp_pcm(self.published);
        let times: [u64; 128] = std::array::from_fn(|offset| {
            1_000_000_000 + (self.published + offset as u64) * 1_000_000_000 / 48_000
        });
        // SAFETY: the fixture exclusively owns this producer; both spans contain 128 frames.
        unsafe {
            assert!(mr_application_source_transport_write_clocked(
                self.application.owner,
                pcm.as_ptr(),
                times.as_ptr(),
                128
            ));
        }
        self.published += 128;
    }
}
impl ExternalIo for Racing<'_, '_> {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        self.capture.source_epoch(binding)
    }
    fn begin_cycle(
        &mut self,
        cycle: &ProcessCycle,
        controls: mixrelay_render::ExternalControlState<'_>,
    ) -> mixrelay_render::CycleAdmission {
        if std::mem::take(&mut self.race) {
            self.publish();
        }
        self.capture.begin_cycle(cycle, controls)
    }
    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        output: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.capture.read_source(binding, output, cycle)
    }
    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        samples: &[f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.capture.write_sink(binding, samples, cycle)
    }
}

/// Every frame carries a distinct nonzero value, so silence and offsets are visible.
fn ramp_pcm(first: u64) -> [f32; 256] {
    std::array::from_fn(|index| {
        #[expect(
            clippy::cast_precision_loss,
            reason = "fixture frame IDs fit exactly within Float32"
        )]
        let value = (first + index as u64 / 2 + 1) as f32 / 16384.0;
        if index % 2 == 0 { value } else { -value }
    })
}
