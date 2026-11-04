//! Counters the test graph driver publishes each native cycle, read back as JSON evidence.
use super::*;

pub(super) struct DriverCounters {
    #[cfg(feature = "native-lab")]
    pub(super) cycles: AtomicU64,
    pub(super) frames: AtomicU64,
    pub(super) clock: AtomicU64,
    pub(super) position: AtomicU64,
    pub(super) discontinuities: AtomicU64,
    pub(super) clock_diagnostics: Arc<crate::processing::clock::DiscontinuityMailbox>,
    pub(super) transfers: [[AtomicU64; mixrelay_linux_pipewire::TRANSFER_SNAPSHOT_WORDS]; 3],
    pub(super) transfer_generation: AtomicU64,
}

impl Default for DriverCounters {
    fn default() -> Self {
        Self {
            #[cfg(feature = "native-lab")]
            cycles: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            clock: AtomicU64::new(0),
            position: AtomicU64::new(0),
            discontinuities: AtomicU64::new(0),
            clock_diagnostics: Arc::default(),
            transfers: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            transfer_generation: AtomicU64::new(0),
        }
    }
}

impl DriverCounters {
    pub(super) fn transfer_observation(&self) -> Value {
        self.transfer_observation_with(|| {})
    }

    // This hook deterministically replaces an owner between the payload and
    // version reads in unit tests; the actual control reader passes a no-op.
    pub(super) fn transfer_observation_with(&self, mut after_payload: impl FnMut()) -> Value {
        const MAX_ATTEMPTS: u32 = 3;
        let native_cycle = |cycle: &mixrelay_linux_pipewire::DriverCycle| {
            serde_json::json!({"clock_id": cycle.clock_id, "rate": cycle.rate,
                "position": cycle.position, "frames": cycle.frames,
                "monotonic_ns": cycle.monotonic_ns})
        };
        let latest_clock_discontinuity = self.clock_diagnostics.read_since(0).map(|read| {
            serde_json::json!({"previous": native_cycle(&read.event.previous),
                "current": native_cycle(&read.event.current),
                "mapped_epoch": read.event.mapped_epoch,
                "render_position": read.event.render_position,
                "event_number": read.event_number, "overwritten": read.overwritten})
        });
        // This latest-only read is separate from the owner snapshot. Counts are lifetime
        // totals, and null means no event or an overlapping writer; reads do not consume it.
        let mailbox_contract = "separate latest control read; counts lifetime events";
        for attempt in 1..=MAX_ATTEMPTS {
            let generation = self.transfer_generation.load(Ordering::SeqCst);
            if generation & 1 != 0 {
                continue;
            }
            let sets: [mixrelay_linux_pipewire::TransferSnapshot; 3] = std::array::from_fn(|set| {
                mixrelay_linux_pipewire::TransferSnapshot::from_words(&std::array::from_fn(
                    |word| self.transfers[set][word].load(Ordering::SeqCst),
                ))
            });
            let clock = self.clock.load(Ordering::SeqCst);
            let position = self.position.load(Ordering::SeqCst);
            let frames = self.frames.load(Ordering::SeqCst);
            let discontinuities = self.discontinuities.load(Ordering::SeqCst);
            after_payload();
            if self.transfer_generation.load(Ordering::SeqCst) == generation {
                return serde_json::json!({
                    "available": true, "generation": generation, "attempts": attempt,
                    "engineClock": {"id": clock >> 32,
                        "rate": u32::try_from(clock & u64::from(u32::MAX))
                            .expect("masked native clock rate fits u32"),
                        "position": position, "callbackFrames": frames,
                        "discontinuities": discontinuities},
                    "capture": sets[0], "endpoint": sets[1], "device": sets[2],
                    "latestClockDiscontinuity": latest_clock_discontinuity,
                    "clockDiscontinuityContract": mailbox_contract,
                });
            }
        }
        serde_json::json!({"available": false, "attempts": MAX_ATTEMPTS,
            "reason": "test owner publication changed or remained in progress",
            "latestClockDiscontinuity": latest_clock_discontinuity,
            "clockDiscontinuityContract": mailbox_contract})
    }
}

#[cfg(feature = "native-lab")]
pub(super) struct LinuxGraphProcess<'a> {
    pub(super) state: crate::processing::LinuxProcessState<'a>,
    pub(super) counters: Arc<DriverCounters>,
}

#[cfg(feature = "native-lab")]
impl mixrelay_linux_pipewire::DriverProcess for LinuxGraphProcess<'_> {
    fn prepare_inputs(
        &mut self,
        inputs: &mixrelay_linux_pipewire::GraphInputs,
    ) -> Result<(), mixrelay_linux_pipewire::DriverError> {
        self.state.prepare_inputs(inputs)
    }

    fn process(&mut self, cycle: &mixrelay_linux_pipewire::DriverCycle) {
        self.state.process(cycle);
        // These test-only loads borrow current owners after their normal report
        // work. No native pointer, report baseline or PCM cursor escapes.
        let sets = self.state.transfer_snapshots();
        // This exclusive DriverProcess writer publishes a fixed bundle. Markers
        // AND payload use SeqCst: ARM64 has the same SC order, with odd before
        // changed words and even after them. A reader observing changed words
        // cannot validate the old even marker. This protects owner labels, not
        // the independently observed C queue values inside the bundle.
        self.counters
            .transfer_generation
            .fetch_add(1, Ordering::SeqCst);
        self.counters.cycles.fetch_add(1, Ordering::SeqCst);
        self.counters
            .frames
            .fetch_add(u64::from(cycle.frames), Ordering::SeqCst);
        self.counters.discontinuities.store(
            self.state
                .clock_diagnostics()
                .read_since(0)
                .map_or(0, |read| read.event_number),
            Ordering::SeqCst,
        );
        self.counters.clock.store(
            u64::from(cycle.clock_id) << 32 | u64::from(cycle.rate),
            Ordering::SeqCst,
        );
        for (destination, snapshot) in self.counters.transfers.iter().zip(sets) {
            for (word, value) in destination.iter().zip(snapshot.words()) {
                word.store(value, Ordering::SeqCst);
            }
        }
        self.counters
            .position
            .store(cycle.position, Ordering::SeqCst);
        self.counters
            .transfer_generation
            .fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(feature = "native-lab")]
pub(super) struct DriverStats {
    pub(super) cycles: u64,
    pub(super) frames: u64,
    pub(super) clock: u64,
}

#[cfg(feature = "native-lab")]
impl DriverStats {
    #[cfg(feature = "native-lab")]
    pub(super) fn sample_rate(&self) -> u32 {
        u32::try_from(self.clock & u64::from(u32::MAX)).expect("masked clock rate fits u32")
    }
}

#[cfg(feature = "native-lab")]
pub(super) fn read_driver_counters(counters: &DriverCounters) -> DriverStats {
    DriverStats {
        cycles: counters.cycles.load(Ordering::Acquire),
        frames: counters.frames.load(Ordering::Acquire),
        clock: counters.clock.load(Ordering::Acquire),
    }
}

#[test]
pub(super) fn transfer_observation_retains_original_latest_clock_transition_separately() {
    let counters = DriverCounters::default();
    assert!(counters.transfer_observation()["latestClockDiscontinuity"].is_null());
    let previous = mixrelay_linux_pipewire::DriverCycle {
        clock_id: 26,
        rate: 48_000,
        position: 4096,
        frames: 128,
        monotonic_ns: 1_000_000,
    };
    let current = mixrelay_linux_pipewire::DriverCycle {
        clock_id: 27,
        rate: 44_100,
        position: 4480,
        frames: 256,
        monotonic_ns: 9_000_000,
    };
    let event = crate::processing::clock::NativeDiscontinuity {
        previous,
        current,
        mapped_epoch: 3,
        render_position: 8192,
    };
    counters
        .clock_diagnostics
        .publish(crate::processing::clock::NativeDiscontinuity {
            previous: mixrelay_linux_pipewire::DriverCycle {
                position: 3840,
                ..previous
            },
            current: previous,
            mapped_epoch: 2,
            render_position: 8064,
        });
    counters.clock_diagnostics.publish(event);
    let observation = counters.transfer_observation();
    assert_eq!(
        observation["latestClockDiscontinuity"],
        serde_json::json!({
            "previous": {"clock_id": 26, "rate": 48_000, "position": 4096,
                "frames": 128, "monotonic_ns": 1_000_000},
            "current": {"clock_id": 27, "rate": 44_100, "position": 4480,
                "frames": 256, "monotonic_ns": 9_000_000},
            "mapped_epoch": 3, "render_position": 8192,
            "event_number": 2, "overwritten": 1,
        })
    );
    assert_eq!(observation["generation"], 0);
    assert_eq!(observation["engineClock"]["discontinuities"], 0);
    assert_eq!(
        counters.transfer_observation()["latestClockDiscontinuity"],
        observation["latestClockDiscontinuity"]
    );
    counters.transfer_generation.store(1, Ordering::SeqCst);
    let unavailable = counters.transfer_observation();
    assert_eq!(unavailable["available"], false);
    assert_eq!(
        unavailable["latestClockDiscontinuity"],
        observation["latestClockDiscontinuity"]
    );
}

#[test]
pub(super) fn transfer_observation_keeps_owned_sets_and_engine_clock_separate() {
    let counters = DriverCounters::default();
    let empty = counters.transfer_observation();
    assert_eq!(empty["capture"]["streams"], 0);
    for (set_index, set) in counters.transfers.iter().enumerate() {
        set[0].store(1, Ordering::Relaxed);
        set[4].store(set_index as u64 + 2, Ordering::Relaxed);
        set[8].store((10 + set_index as u64) << 32 | 0xbb80, Ordering::Relaxed);
        set[28].store(set_index as u64 + 1, Ordering::Relaxed);
        set[29].store((20 + set_index as u64) << 32 | 0xbb80, Ordering::Relaxed);
        set[30].store(256_u64 << 32 | 128, Ordering::Relaxed);
        set[31].store(1024, Ordering::Relaxed);
        set[32].store(2, Ordering::Relaxed);
        set[33].store(128, Ordering::Relaxed);
        set[34].store(3, Ordering::Relaxed);
    }
    counters
        .clock
        .store(99_u64 << 32 | 0xbb80, Ordering::Release);
    counters.position.store(4096, Ordering::Relaxed);
    counters.frames.store(8192, Ordering::SeqCst);
    counters.discontinuities.store(2, Ordering::SeqCst);
    let observation = counters.transfer_observation();
    for (index, name) in ["capture", "endpoint", "device"].iter().enumerate() {
        assert_eq!(observation[name]["streams"], 1);
        assert_eq!(observation[name]["owner_sequence"], index as u64 + 1);
        assert_eq!(
            observation[name]["native_clock"],
            (20 + index as u64) << 32 | 0xbb80
        );
        assert_eq!(observation[name]["native_quantum"], 256_u64 << 32 | 128);
        assert_eq!(observation[name]["counters"]["engine_read_frames"], 1024);
        assert_eq!(observation[name]["counters"]["consumer_discard_calls"], 2);
        assert_eq!(
            observation[name]["counters"]["consumer_discard_frames"],
            128
        );
        assert_eq!(observation[name]["counters"]["native_restart_calls"], 3);
        assert_eq!(
            observation[name]["counters"]["failures"][0]["rejected_calls"],
            index as u64 + 2
        );
        assert_ne!(
            observation[name]["counters"]["failures"][0]["first_clock"],
            99_u64 << 32 | 0xbb80
        );
    }
    assert_eq!(observation["engineClock"]["id"], 99);
    assert_eq!(observation["engineClock"]["rate"], 48_000);
    assert_eq!(observation["engineClock"]["position"], 4096);
    assert_eq!(observation["engineClock"]["callbackFrames"], 8192);
    assert_eq!(observation["engineClock"]["discontinuities"], 2);
    let mut published = false;
    let fresh = counters.transfer_observation_with(|| {
        if !published {
            counters.transfer_generation.fetch_add(1, Ordering::SeqCst);
            counters.frames.store(16_384, Ordering::SeqCst);
            counters.transfer_generation.fetch_add(1, Ordering::SeqCst);
            published = true;
        }
    });
    assert_eq!(fresh["attempts"], 2);
    assert_eq!(fresh["engineClock"]["callbackFrames"], 16_384);
}

#[test]
pub(super) fn converted_clock_window_keeps_fraction_and_excludes_lifetime_failures() {
    let rate = 44_100;
    let counters = DriverCounters::default();
    counters
        .clock
        .store(7_u64 << 32 | u64::from(rate), Ordering::SeqCst);
    let output = &counters.transfers[2];
    output[0].store(1, Ordering::SeqCst);
    output[28].store(3, Ordering::SeqCst);
    output[29].store(7_u64 << 32 | u64::from(rate), Ordering::SeqCst);
    // Warm-up failures must not be charged again to the settled PCM window.
    output[10].store(5, Ordering::SeqCst);
    output[11].store(256, Ordering::SeqCst);
    counters.frames.store(128, Ordering::SeqCst);
    output[2].store(139, Ordering::SeqCst);
    let before = counters.transfer_observation();
    counters.frames.store(256, Ordering::SeqCst);
    output[2].store(278, Ordering::SeqCst);
    let after = counters.transfer_observation();
    let mut window = serde_json::json!({"before": before, "after": after});
    assert_clock_window(&window, rate);
    // The next carried fraction may legitimately produce 140 instead of 139.
    window["after"]["device"]["counters"]["written"] = serde_json::json!(279);
    assert_clock_window(&window, rate);
    window["after"]["device"]["counters"]["written"] = serde_json::json!(407);
    assert!(std::panic::catch_unwind(|| assert_clock_window(&window, rate)).is_err());
    window["after"]["device"]["counters"]["written"] = serde_json::json!(278);
    window["after"]["device"]["counters"]["native_restart_calls"] = serde_json::json!(1);
    assert!(std::panic::catch_unwind(|| assert_clock_window(&window, rate)).is_err());
}

pub(super) fn counter_delta(before: &Value, after: &Value) -> u64 {
    after
        .as_u64()
        .expect("unsigned after counter")
        .wrapping_sub(before.as_u64().expect("unsigned before counter"))
}

pub(super) fn assert_clock_window(window: &Value, native_rate: u32) {
    let before = &window["before"];
    let after = &window["after"];
    for observation in [before, after] {
        assert_eq!(observation["available"], true, "coherent clock publication");
        assert_eq!(
            observation["device"]["streams"], 1,
            "one graph output owner"
        );
        assert_eq!(observation["engineClock"]["rate"], native_rate);
        let clock_id = observation["engineClock"]["id"].as_u64().unwrap();
        assert_ne!(clock_id, 0, "observed native graph clock");
        assert_eq!(
            observation["device"]["native_clock"],
            clock_id << 32 | u64::from(native_rate)
        );
    }
    assert_ne!(before["device"]["owner_sequence"], 0);
    assert_eq!(
        before["device"]["owner_sequence"],
        after["device"]["owner_sequence"]
    );
    assert_eq!(before["engineClock"]["id"], after["engineClock"]["id"]);
    assert_eq!(
        counter_delta(
            &before["engineClock"]["discontinuities"],
            &after["engineClock"]["discontinuities"]
        ),
        0,
        "native callback timeline changed within the duration window"
    );
    let initial = &before["device"]["counters"];
    let current = &after["device"]["counters"];
    for field in [
        "native_restart_calls",
        "consumer_discard_calls",
        "consumer_discard_frames",
    ] {
        assert_eq!(
            counter_delta(&initial[field], &current[field]),
            0,
            "{field}"
        );
    }
    for stage in 0..4 {
        for field in ["rejected_calls", "rejected_frames"] {
            assert_eq!(
                counter_delta(
                    &initial["failures"][stage][field],
                    &current["failures"][stage][field]
                ),
                0,
                "output stage {stage} {field} within duration window"
            );
        }
    }
    let callback_frames = counter_delta(
        &before["engineClock"]["callbackFrames"],
        &after["engineClock"]["callbackFrames"],
    );
    let written = counter_delta(&initial["written"], &current["written"]);
    assert!(callback_frames > 0 && written > 0, "no processed duration");
    // Output-ring written advances only from graph writes, never native reads.
    // Difference of two carried-remainder floors is strictly below one engine frame.
    let frame_error = (i128::from(written) * i128::from(native_rate)
        - i128::from(callback_frames) * i128::from(SAMPLE_RATE))
    .abs();
    assert!(
        frame_error < i128::from(native_rate),
        "native={callback_frames} written={written} rate={native_rate} frame error numerator={frame_error}"
    );
    println!(
        "native_duration native_rate={native_rate} callback_frames={callback_frames} \
         engine_rate={SAMPLE_RATE} written_frames={written} \
         frame_error_numerator={frame_error} frame_error_denominator={native_rate}"
    );
}
