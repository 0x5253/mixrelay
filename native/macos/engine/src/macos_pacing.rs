//! Native frame-credit pacing. Sleeping only polls for new credits; elapsed wall
//! time cannot manufacture a device callback, sample position, or host timestamp.
use mixrelay_macos_transport::source_registry::PublishedCredit;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NativeClockSnapshot {
    pub incarnation: [u8; 16],
    pub epoch: u64,
    pub render_through: u64,
    pub capture_through: u64,
    pub sample_rate: u32,
    pub running: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ApplicationClockSnapshot {
    pub epoch: u64,
    pub published_frames: u64,
    pub native_rate: u32,
    pub graph_rate: u32,
    pub running: u32,
    pub synchronized: u32,
    pub capacity_frames: u32,
}

#[derive(Default)]
pub(super) struct ApplicationPacer {
    master: Option<(u64, u64)>,
    awaited_source: Option<u64>,
    /// The source consumed from its first credit: awaited, then each new epoch of it.
    head: Option<u64>,
    generation: u64,
    hardware: HardwarePacer,
}

impl ApplicationPacer {
    /// Arms a graph that is already consuming before its producer becomes live.
    pub(super) fn await_source(&mut self, token: u64) {
        self.awaited_source = Some(token);
        self.head = Some(token);
    }

    /// The source this pacer consumes from its first credit, which alone may receive
    /// PCM its producer retained before admission.
    pub(super) const fn head(&self) -> Option<u64> {
        self.head
    }

    pub(super) fn next(
        &mut self,
        source: (u64, ApplicationClockSnapshot),
        format: (u32, u32, u32),
    ) -> Pace {
        let (token, clock) = source;
        let (rate, maximum, capacity) = format;
        if clock.running == 0 {
            self.await_source(token);
            return Pace::Idle;
        }
        // The graph consumer must publish its new format to the producer. Until
        // acknowledgment, the worker can run that handshake; waiting here
        // would deadlock an application-only graph after a sample-rate change.
        if clock.graph_rate != rate || clock.native_rate == 0 {
            return Pace::Unknown;
        }
        let awaited = self.awaited_source.take() == Some(token);
        if self.master != Some((token, clock.epoch)) {
            let continuing = self.master.is_some_and(|(previous, _)| previous == token);
            if awaited {
                self.head = Some(token);
            } else if !continuing {
                // A source joined while already running starts at its current credit.
                self.head = None;
            }
            self.master = Some((token, clock.epoch));
            self.generation = self.generation.wrapping_add(1);
            if awaited || continuing {
                // A new producer epoch starts at zero. Retain the input published
                // while this active consumer was waiting for its first observation.
                self.hardware.begin((self.generation, 0, rate), rate);
            }
        }
        let mut pace = self.hardware.next(
            (self.generation, clock.published_frames, rate),
            rate,
            maximum,
            capacity,
        );
        if let Pace::Block { publication, .. } = &mut pace {
            *publication = Some(Publication {
                token,
                credit: PublishedCredit {
                    epoch: clock.epoch,
                    through: clock.published_frames,
                },
            });
        }
        pace
    }
}

/// The shared-source publication snapshot that granted a block's credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Publication {
    pub token: u64,
    pub credit: PublishedCredit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pace {
    Unknown,
    Idle,
    RateMismatch,
    Wait,
    Block {
        first_frame: u64,
        frames: u32,
        /// Remaining graph-frame credit, including this block.
        available_frames: u32,
        epoch: u64,
        discontinuity: bool,
        overrun: bool,
        /// The pacing source's credited publication; device clocks have none.
        publication: Option<Publication>,
    },
}

#[derive(Default)]
pub(super) struct NativePacer {
    epoch: Option<([u8; 16], u64)>,
    next_frame: u64,
}

#[derive(Clone, Copy)]
struct CreditWindow {
    maximum: u32,
    capacity: u32,
    retained: u32,
}

/// Projects observed hardware-frame credit into graph frames with integer
/// arithmetic. Fractional credit is retained by recomputing from the baseline,
/// rather than rounding each callback independently. SRC owns the PCM samples.
#[derive(Default)]
pub(super) struct HardwarePacer {
    origin: Option<(u64, u64, u32, u32)>,
    pacing: NativePacer,
}

impl HardwarePacer {
    fn begin(&mut self, (epoch, baseline, native_rate): (u64, u64, u32), graph_rate: u32) {
        self.origin = Some((epoch, baseline, native_rate, graph_rate));
        self.pacing.reset();
    }

    pub(super) fn next(
        &mut self,
        progress: (u64, u64, u32),
        graph_rate: u32,
        maximum_frames: u32,
        capacity: u32,
    ) -> Pace {
        let (epoch, native_frames, native_rate) = progress;
        if native_rate == 0 {
            return Pace::Unknown;
        }
        let (previous_epoch, baseline, previous_rate, previous_graph_rate) = self
            .origin
            .unwrap_or((epoch, native_frames, native_rate, graph_rate));
        if self.origin.is_none()
            || previous_epoch != epoch
            || native_frames < baseline
            || previous_rate != native_rate
            || previous_graph_rate != graph_rate
        {
            self.begin((epoch, native_frames, native_rate), graph_rate);
            return Pace::Wait;
        }
        let through = u64::try_from(
            u128::from(native_frames - baseline) * u128::from(graph_rate) / u128::from(native_rate),
        )
        .unwrap_or(u64::MAX);
        if through == 0 {
            return Pace::Wait;
        }
        self.pacing.next(
            Some(NativeClockSnapshot {
                incarnation: [0; 16],
                epoch,
                render_through: through,
                capture_through: 0,
                sample_rate: graph_rate,
                running: 1,
            }),
            graph_rate,
            maximum_frames,
            capacity,
        )
    }
}

impl NativePacer {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn next(
        &mut self,
        observation: Option<NativeClockSnapshot>,
        rate: u32,
        maximum_frames: u32,
        capacity: u32,
    ) -> Pace {
        self.advance(
            observation,
            rate,
            CreditWindow {
                maximum: maximum_frames,
                capacity,
                retained: maximum_frames,
            },
        )
    }

    fn advance(
        &mut self,
        observation: Option<NativeClockSnapshot>,
        rate: u32,
        window: CreditWindow,
    ) -> Pace {
        let maximum_frames = window.maximum;
        let capacity = window.capacity;
        let Some(clock) = observation else {
            self.reset();
            return Pace::Unknown;
        };
        if clock.running == 0 {
            self.reset();
            return Pace::Idle;
        }
        if clock.sample_rate != rate {
            self.reset();
            return Pace::RateMismatch;
        }
        let through = clock.render_through.max(clock.capture_through);
        if through == 0 {
            // Running with no published callback is priming, not a stopped clock.
            // Keep the selected owner; the first real credit establishes its epoch.
            return Pace::Wait;
        }
        let identity = (clock.incarnation, clock.epoch);
        let restarted = self.epoch != Some(identity);
        if restarted {
            self.epoch = Some(identity);
            // Late attachment starts within the retained window, never replaying
            // the lifetime history or the preceding engine-timer timeline.
            self.next_frame = if through <= u64::from(capacity) {
                0
            } else {
                through.saturating_sub(u64::from(window.retained))
            };
        }
        let lost_window = through.saturating_sub(self.next_frame) > u64::from(capacity);
        if lost_window {
            self.next_frame = through.saturating_sub(u64::from(window.retained));
        }
        let available = through.saturating_sub(self.next_frame);
        let frames = u32::try_from(available)
            .unwrap_or(u32::MAX)
            .min(maximum_frames);
        if frames == 0 {
            return Pace::Wait;
        }
        let first_frame = self.next_frame;
        self.next_frame += u64::from(frames);
        Pace::Block {
            first_frame,
            frames,
            available_frames: u32::try_from(available).unwrap_or(u32::MAX),
            epoch: clock.epoch,
            discontinuity: restarted || lost_window,
            overrun: lost_window,
            publication: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(render: u64, capture: u64) -> NativeClockSnapshot {
        NativeClockSnapshot {
            incarnation: [1; 16],
            epoch: 1,
            render_through: render,
            capture_through: capture,
            sample_rate: 48_000,
            running: 1,
        }
    }

    #[test]
    fn unknown_idle_and_mismatch_never_create_device_frames() {
        let mut pacer = NativePacer::default();
        assert_eq!(pacer.next(None, 48_000, 128, 4096), Pace::Unknown);
        assert_eq!(
            pacer.next(Some(NativeClockSnapshot::default()), 48_000, 128, 4096),
            Pace::Idle
        );
        assert_eq!(
            pacer.next(Some(snapshot(0, 0)), 48_000, 128, 4096),
            Pace::Wait
        );
        assert_eq!(
            pacer.next(Some(snapshot(128, 0)), 44_100, 128, 4096),
            Pace::RateMismatch
        );
    }

    #[test]
    fn render_and_capture_only_use_real_variable_frame_credits() {
        for capture_only in [false, true] {
            let mut pacer = NativePacer::default();
            let mut through = 0;
            let mut processed = 0;
            for quantum in [64, 96, 257, 511, 128] {
                through += quantum;
                let clock = if capture_only {
                    snapshot(0, through)
                } else {
                    snapshot(through, 0)
                };
                loop {
                    match pacer.next(Some(clock), 48_000, 128, 4096) {
                        Pace::Block {
                            first_frame,
                            frames,
                            available_frames,
                            ..
                        } => {
                            assert_eq!(u64::from(available_frames), through - processed);
                            assert_eq!(first_frame, processed);
                            assert!(frames <= 128);
                            processed += u64::from(frames);
                        }
                        Pace::Wait => break,
                        other => panic!("unexpected {other:?}"),
                    }
                }
                for _ in 0..375 {
                    assert_eq!(pacer.next(Some(clock), 48_000, 128, 4096), Pace::Wait);
                }
                assert_eq!(processed, through);
            }
            assert_eq!(processed, 1056);
        }
    }

    #[test]
    fn restarts_and_late_attachment_never_replay_old_positions() {
        let mut pacer = NativePacer::default();
        assert_eq!(
            pacer.next(Some(snapshot(10000, 0)), 48_000, 128, 4096),
            Pace::Block {
                first_frame: 9872,
                frames: 128,
                available_frames: 128,
                epoch: 1,
                discontinuity: true,
                overrun: false,
                publication: None,
            }
        );
        let restarted = NativeClockSnapshot {
            epoch: 2,
            ..snapshot(64, 0)
        };
        assert_eq!(
            pacer.next(Some(restarted), 48_000, 128, 4096),
            Pace::Block {
                first_frame: 0,
                frames: 64,
                available_frames: 64,
                epoch: 2,
                discontinuity: true,
                overrun: false,
                publication: None,
            }
        );
        let overrun = NativeClockSnapshot {
            epoch: 2,
            ..snapshot(10000, 0)
        };
        assert_eq!(
            pacer.next(Some(overrun), 48_000, 128, 4096),
            Pace::Block {
                first_frame: 9872,
                frames: 128,
                available_frames: 128,
                epoch: 2,
                discontinuity: true,
                overrun: true,
                publication: None,
            }
        );
    }

    #[test]
    fn hardware_credit_preserves_fractional_ratios_and_restarts() {
        let mut pacer = HardwarePacer::default();
        assert_eq!(pacer.next((1, 100, 44_100), 48_000, 128, 4096), Pace::Wait);
        let mut total = 0;
        for native in 101..=44_200 {
            while let Pace::Block {
                first_frame,
                frames,
                ..
            } = pacer.next((1, native, 44_100), 48_000, 128, 4096)
            {
                assert_eq!(first_frame, total);
                total += u64::from(frames);
            }
        }
        assert_eq!(total, 48_000);
        assert_eq!(pacer.next((2, 100, 44_100), 48_000, 128, 4096), Pace::Wait);
        assert_eq!(pacer.next((2, 200, 48_000), 48_000, 128, 4096), Pace::Wait);
        assert_eq!(pacer.next((2, 300, 48_000), 44_100, 128, 4096), Pace::Wait);
        assert_eq!(pacer.next((2, 0, 48_000), 44_100, 128, 4096), Pace::Wait);
        assert_eq!(pacer.next((2, 0, 0), 44_100, 128, 4096), Pace::Unknown);
    }

    #[test]
    fn unchanged_native_credit_does_not_advance_like_the_old_timer() {
        let mut pacer = NativePacer::default();
        let clock = snapshot(128, 0);
        assert!(matches!(
            pacer.next(Some(clock), 48_000, 128, 4096),
            Pace::Block { frames: 128, .. }
        ));
        let mut old_timer_frames = 0;
        let mut credited_frames = 0;
        for _ in 0..375 {
            old_timer_frames += 128;
            match pacer.next(Some(clock), 48_000, 128, 4096) {
                Pace::Block { frames, .. } => credited_frames += frames,
                Pace::Wait => (),
                unexpected => panic!("unexpected {unexpected:?}"),
            }
        }
        assert_eq!((old_timer_frames, credited_frames), (48_000, 0));
        eprintln!(
            "375 unchanged-credit observations: old timer advanced {old_timer_frames}; native pacer advanced {credited_frames}"
        );
    }

    #[test]
    fn new_mapping_with_equal_local_epoch_resets_worker_credit_identity() {
        let mut pacer = NativePacer::default();
        let clock = snapshot(128, 0);
        assert!(matches!(
            pacer.next(Some(clock), 48_000, 128, 4096),
            Pace::Block {
                discontinuity: true,
                ..
            }
        ));
        assert_eq!(pacer.next(Some(clock), 48_000, 128, 4096), Pace::Wait);
        let replaced = NativeClockSnapshot {
            incarnation: [2; 16],
            ..clock
        };
        assert!(matches!(
            pacer.next(Some(replaced), 48_000, 128, 4096),
            Pace::Block {
                first_frame: 0,
                frames: 128,
                discontinuity: true,
                ..
            }
        ));
        assert_eq!(pacer.next(Some(replaced), 48_000, 128, 4096), Pace::Wait);
    }

    #[test]
    fn source_start_after_an_idle_observation_preserves_its_first_retained_credit() {
        let mut pacer = ApplicationPacer::default();
        let format = (48_000, 128, 16_384);
        let mut clock = ApplicationClockSnapshot {
            epoch: 7,
            published_frames: 0,
            native_rate: 48_000,
            graph_rate: 48_000,
            running: 0,
            synchronized: 1,
            capacity_frames: 16_384,
        };
        assert_eq!(pacer.next((11, clock), format), Pace::Idle);
        clock.published_frames = 8_192;
        clock.running = 1;
        clock.native_rate = 0;
        assert_eq!(pacer.next((11, clock), format), Pace::Unknown);
        clock.native_rate = 48_000;
        let mut received = 0;
        for _ in 0..64 {
            let Pace::Block {
                frames,
                publication,
                ..
            } = pacer.next((11, clock), format)
            else {
                break;
            };
            // Every block names the exact publication snapshot that granted it.
            assert_eq!(
                publication,
                Some(Publication {
                    token: 11,
                    credit: PublishedCredit {
                        epoch: 7,
                        through: 8_192,
                    },
                })
            );
            received += frames;
        }
        assert_eq!(
            received, 8_192,
            "the active consumer lost the producer's startup PCM"
        );
        assert_eq!(pacer.next((11, clock), format), Pace::Wait);
    }

    #[test]
    fn a_continuing_source_new_epoch_preserves_only_its_new_native_credit() {
        let mut pacer = ApplicationPacer::default();
        let format = (48_000, 128, 16_384);
        let mut clock = ApplicationClockSnapshot {
            epoch: 7,
            published_frames: 8_192,
            native_rate: 48_000,
            graph_rate: 48_000,
            running: 1,
            synchronized: 1,
            capacity_frames: 16_384,
        };
        // Attaching to an already running source still excludes its pre-admission history.
        assert_eq!(pacer.next((11, clock), format), Pace::Wait);
        clock.epoch = 8;
        clock.published_frames = 384;
        for first in [0, 128, 256] {
            assert!(matches!(pacer.next((11, clock), format), Pace::Block {
                first_frame, frames: 128, overrun: false, ..
            } if first_frame == first));
        }
        assert_eq!(pacer.next((11, clock), format), Pace::Wait);
    }

    #[test]
    fn application_master_tracks_identity_epoch_and_format_handshake() {
        let mut pacer = ApplicationPacer::default();
        let format = (48_000, 128, 4096);
        let mut clock = ApplicationClockSnapshot {
            epoch: 1,
            published_frames: 100,
            native_rate: 44_100,
            graph_rate: 48_000,
            running: 1,
            synchronized: 1,
            capacity_frames: 4096,
        };
        assert_eq!(pacer.next((11, clock), format), Pace::Wait);
        clock.published_frames += 480;
        let mut processed = 0;
        while let Pace::Block { frames, .. } = pacer.next((11, clock), format) {
            processed += frames;
        }
        assert_eq!(processed, 480);
        assert_eq!(pacer.next((12, clock), format), Pace::Wait);
        clock.epoch += 1;
        clock.published_frames = 0;
        assert_eq!(pacer.next((12, clock), format), Pace::Wait);
        clock.graph_rate = 44_100;
        assert_eq!(pacer.next((12, clock), format), Pace::Unknown);
        clock.running = 0;
        assert_eq!(pacer.next((12, clock), format), Pace::Idle);
    }
}
