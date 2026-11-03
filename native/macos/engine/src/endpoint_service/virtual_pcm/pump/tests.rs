//! Drives real pump cycles against HAL-shaped client queues and a graph source, with
//! a synthetic host clock and graph consumers emulated through the source registry.
use super::*;
use mixrelay_audio_io::{
    capture_alignment::{CaptureAlignment, CaptureFrontier},
    process::{ClockDomain, ProcessCycle},
    session::ClockAuthority,
};
use mixrelay_macos_transport::{
    application_clock::VERSION,
    client_pcm::{ClientPcmSnapshot, ClientPcmWriter},
    endpoint_transport::EndpointTransportConfig,
    source_control::SourceRequest,
    source_registry::{GraphCredit, ReadMiss, SourceRegistry, content_latency::ReadTiming},
};
use std::{sync::atomic::AtomicU64, time::Duration};

/// The registry's consumer scheduling clock follows the fixture's native playhead.
struct GraphClock {
    nanos: Arc<AtomicU64>,
}
impl mixrelay_time::MonotonicClock for GraphClock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::from_nanos(
            self.nanos.load(Ordering::Relaxed),
        ))
    }
}

const SOURCE: &str = "virtual:fixture";

fn source_token() -> u64 {
    SOURCE
        .bytes()
        .fold(14_695_981_039_346_656_037, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
        })
}

/// A frame's identity survives mixing, retention and same-clock conversion bit for bit.
fn codeword(frame: u64) -> f32 {
    f32::from_bits(0x3f00_0000 + u32::try_from(frame).unwrap())
}

/// Field order is drop order: the pump releases its source lease before the registry.
struct Fixture {
    pump: Pump,
    writer: Option<ClientPcmWriter>,
    registry: SourceRegistry,
    scheduling: Scheduling,
    timebase: Timebase,
    graph_nanos: Arc<AtomicU64>,
    rate: u32,
    anchor: u64,
    /// Native frames the client has published on the current timeline.
    written: u64,
}

impl Fixture {
    fn new(rate: u32) -> Self {
        let timebase = Timebase::current();
        let anchor = timebase.ticks(Duration::from_secs(1));
        let mut writer = ClientPcmWriter::new(
            EndpointTransportConfig {
                endpoint_id: [1; 16],
                capacity_frames: QUEUE_FRAMES,
                render_channels: 2,
                capture_channels: 2,
            },
            42,
        )
        .unwrap();
        assert!(writer.set_clock((2, anchor), rate, true, true));
        let mut snapshot = ClientPcmSnapshot::new().unwrap();
        assert!(snapshot.push(&writer));
        // SAFETY: the snapshot retains the producer-created grants throughout decoding.
        let queues = unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap();
        let graph_nanos = Arc::new(AtomicU64::new(0));
        let registry =
            crate::endpoint_service::virtual_pcm::test_storage::registry(Box::new(GraphClock {
                nanos: graph_nanos.clone(),
            }));
        let mut request = SourceRequest {
            channels: 2,
            capacity_frames: mixrelay_audio_io::retention::RING_FRAMES,
            transport_version: VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request.source_id.iter_mut().zip(SOURCE.bytes()) {
            *target = std::ffi::c_char::from_ne_bytes([byte]);
        }
        let transport = SourceTransport::create(&registry, &request).unwrap();
        let pump = Pump::prepare(
            queues,
            transport,
            Signals {
                update: Arc::default(),
                updated: Arc::default(),
                stop: Arc::default(),
            },
        )
        .unwrap();
        Self {
            pump,
            writer: Some(writer),
            registry,
            scheduling: Scheduling::standard(),
            timebase,
            graph_nanos,
            rate,
            anchor,
            written: 0,
        }
    }

    fn writer(&mut self) -> &mut ClientPcmWriter {
        self.writer.as_mut().unwrap()
    }

    /// The client publishes one native period of identifiable stereo PCM.
    fn write(&mut self) {
        let first = self.written;
        let pcm: [f32; 256] = std::array::from_fn(|index| {
            let value = codeword(first + index as u64 / 2);
            if index % 2 == 0 { value } else { -value }
        });
        assert!(self.writer().write(first, &pcm, 128));
        self.written += 128;
        self.graph_nanos.store(
            u64::try_from(u128::from(self.written) * 1_000_000_000 / u128::from(self.rate))
                .unwrap(),
            Ordering::Relaxed,
        );
    }

    /// One pump cycle with the playhead a quarter period past the published frames.
    fn cycle(&mut self) {
        let nanos = (u128::from(self.written) + 32) * 1_000_000_000 / u128::from(self.rate);
        let now = self.anchor
            + self
                .timebase
                .ticks(Duration::from_nanos(u64::try_from(nanos).unwrap()));
        self.pump
            .cycle(now, self.timebase, &self.scheduling)
            .unwrap();
    }

    fn period(&mut self) {
        self.write();
        self.cycle();
    }

    /// The graph's active plan publishes how it uses this source.
    fn route(&self, route: ConsumerRoute) {
        assert!(self.registry.set_consumer_route(source_token(), route));
    }

    /// The graph requests its format at the native rate. Only an application-paced
    /// master converts as a same-clock copy; any other consumer gets the sinc bridge.
    fn request(&self, master: bool) {
        let _ = self.registry.source_time(source_token(), self.rate, 128);
        assert!(self.registry.set_clock_master(source_token(), master));
    }

    /// Published graph frames and native credit of the current source epoch.
    fn progress(&self) -> (u64, u64, u64) {
        self.registry
            .pcm_progress(source_token())
            .map_or((0, 0, 0), |progress| {
                (
                    progress.clock.epoch,
                    progress.published_frames,
                    progress.clock.native_frames,
                )
            })
    }

    fn time(&self, frame: u64) -> u64 {
        let offset = u128::from(frame) * 1_000_000_000 / u128::from(self.rate);
        self.timebase.nanoseconds(self.anchor) + u64::try_from(offset).unwrap()
    }

    /// Reads one block at native frame `first` by time, as a graph consumer does.
    fn read(&self, first: u64) -> Result<[f32; 256], ReadMiss> {
        let mut output = [0.0_f32; 256];
        self.registry
            .read_audio_timed(
                source_token(),
                &mut output,
                128,
                ReadTiming {
                    rate: self.rate,
                    first_nanos: Some(self.time(first)),
                    host_ticks: None,
                },
            )
            .map(|_| output)
    }

    /// Reads the block at `first` and requires the client's PCM at its original position.
    fn verify(&self, first: u64) {
        let output = self
            .read(first)
            .unwrap_or_else(|miss| panic!("no PCM at native frame {first}: {miss}"));
        check(&output, first);
    }
}

/// Requires the client's PCM of native frames `first..first + 128`, bit for bit.
fn check(output: &[f32; 256], first: u64) {
    for (index, frame) in output.chunks_exact(2).enumerate() {
        let expected = codeword(first + index as u64);
        assert_eq!(
            (frame[0].to_bits(), frame[1].to_bits()),
            (expected.to_bits(), (-expected).to_bits()),
            "native frame {} moved or was lost",
            first + index as u64
        );
    }
}

/// The client plays while the application-paced graph waits `deferred` for its format,
/// longer than a client queue holds. Admission then publishes the whole head in order
/// and at native coordinates, a bounded number of periods per cycle, while a graph
/// consumer concurrently reads it by time at real-time pace.
fn master_admission(rate: u32, deferred: Duration) {
    let mut fixture = Fixture::new(rate);
    // The plan routes the source to a recorder it paces from its first credit.
    fixture.route(ConsumerRoute::Routed { head: true });
    let periods =
        u64::try_from(deferred.as_nanos() * u128::from(rate) / 1_000_000_000).unwrap() / 128;
    assert!(periods * 128 > u64::from(QUEUE_FRAMES));
    let measured = allocation_counter::measure(|| {
        for _ in 0..periods {
            fixture.period();
        }
    });
    assert_eq!(
        measured.count_total, 0,
        "retention allocated on the RT pump"
    );
    assert_eq!(fixture.pump.backlog.frames(), periods * 128);
    assert_eq!(fixture.progress(), (0, 0, 0));
    fixture.request(true);
    // The admission cycle builds the converter on control; it may allocate there.
    fixture.period();
    let (epoch, mut published, credit) = fixture.progress();
    assert_ne!(epoch, 0);
    assert_eq!((published, credit), (FEED_PERIODS as u64 * 128, published));
    let mut cycles = 1;
    let mut read = 0;
    let mut allocations = 0;
    while fixture.pump.backlog.frames() != 0 {
        fixture.verify(read);
        read += 128;
        fixture.write();
        allocations += allocation_counter::measure(|| fixture.cycle()).count_total;
        let (current, total, credit) = fixture.progress();
        assert_eq!(current, epoch, "catch-up restarted the source stream");
        assert!(
            (1..=FEED_PERIODS as u64 * 128).contains(&(total - published)),
            "one cycle published {} frames",
            total - published
        );
        assert_eq!(credit, total, "credit diverged from published PCM");
        published = total;
        cycles += 1;
    }
    assert_eq!(allocations, 0, "catch-up allocated on the RT pump");
    // Seven periods of catch-up per cycle beyond its own live period.
    assert_eq!(cycles, periods.div_ceil(FEED_PERIODS as u64 - 1));
    for _ in 0..8 {
        fixture.verify(read);
        read += 128;
        fixture.period();
        assert_eq!(fixture.progress().1, fixture.written);
    }
    while read < fixture.written {
        fixture.verify(read);
        read += 128;
    }
    assert_eq!(fixture.pump.losses(), (0, 0));
}

#[test]
fn a_four_second_admission_delay_at_48_khz_gives_the_master_the_whole_head() {
    master_admission(48_000, Duration::from_secs(4));
}

#[test]
fn a_one_and_a_half_second_admission_delay_at_192_khz_gives_the_master_the_whole_head() {
    master_admission(192_000, Duration::from_millis(1_500));
}

/// A hardware-paced graph's capture cursor before and after a virtual source joins.
struct HardwareGraph {
    alignment: CaptureAlignment,
    /// Nanoseconds per hardware frame.
    period: f64,
    /// Host nanoseconds of the hardware playhead.
    playhead: f64,
    frame: u64,
    cursor: Option<u64>,
    rewind: u64,
    reads: u64,
    misses: u64,
    joined: bool,
}
/// The hardware input publishes its capture 256 frames behind its own playhead.
const HARDWARE_LAG: f64 = 256.0;

impl HardwareGraph {
    fn new(fixture: &Fixture) -> Self {
        #[expect(clippy::cast_precision_loss, reason = "host nanoseconds below 2^52")]
        let playhead = fixture.time(0) as f64;
        Self {
            alignment: CaptureAlignment::default(),
            period: 1e9 / f64::from(fixture.rate),
            playhead,
            frame: 0,
            cursor: None,
            rewind: 0,
            reads: 0,
            misses: 0,
            joined: false,
        }
    }

    /// Every hardware block the device clock completed by the native playhead.
    fn run(&mut self, fixture: &Fixture) {
        #[expect(clippy::cast_precision_loss, reason = "host nanoseconds below 2^52")]
        let native = fixture.time(fixture.written) as f64;
        while self.playhead + 128.0 * self.period <= native {
            self.playhead += 128.0 * self.period;
            self.cycle(fixture);
        }
    }

    /// One graph block: the oldest safe frontier of both inputs, then the virtual read.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive host nanoseconds below 2^52"
    )]
    fn cycle(&mut self, fixture: &Fixture) {
        let period = self.period;
        let hardware = CaptureFrontier::new(1, (self.playhead - HARDWARE_LAG * period) as u64);
        // Until the graph admits the source it only observes it; this request admits it.
        let virtual_time = self
            .joined
            .then(|| {
                fixture.registry.source_time_for_credit(
                    source_token(),
                    fixture.rate,
                    GraphCredit {
                        frames: 128,
                        available_frames: 128,
                        published: None,
                    },
                )
            })
            .flatten();
        let frontier = virtual_time
            .filter(|time| *time < hardware.nanos())
            .map_or(hardware, |time| CaptureFrontier::new(source_token(), time));
        let cycle = ProcessCycle {
            clock: ClockDomain {
                authority: ClockAuthority::Device,
                id: 3,
                epoch: 1,
            },
            sample_rate: fixture.rate,
            frames: 128,
            first_frame: self.frame,
            monotonic_ns: None,
            discontinuity: self.frame == 0,
        };
        self.frame += 128;
        let time = self.alignment.begin(cycle, Some(frontier), period).unwrap();
        if let Some(previous) = self.cursor {
            let expected = previous + (128.0 * period).round() as u64;
            // Nanosecond rounding of the continuous coordinate is not a rewind.
            self.rewind = self.rewind.max(expected.saturating_sub(time + 2));
        }
        self.cursor = Some(time);
        if virtual_time.is_none() {
            return;
        }
        let mut output = [0.0_f32; 256];
        match fixture.registry.read_audio_timed(
            source_token(),
            &mut output,
            128,
            ReadTiming {
                rate: fixture.rate,
                first_nanos: Some(time),
                host_ticks: None,
            },
        ) {
            Ok(_) => self.reads += 1,
            // Priming and the first reads at the join may miss; steady state may not.
            Err(_) if self.reads < 16 => {}
            Err(miss) => {
                assert!(
                    !matches!(miss, ReadMiss::Mismatch | ReadMiss::Behind),
                    "steady-state read missed: {miss}"
                );
                self.misses += 1;
            }
        }
    }
}

/// A hardware-paced graph admits a virtual source two seconds after its client started.
/// The source joins at its live edge, and its independent-clock bridge holds twenty
/// seconds of steady state without pinning its correction or missing a read.
#[test]
fn a_hardware_paced_graph_joins_a_late_virtual_source_at_its_live_edge() {
    let rate = 48_000;
    let mut fixture = Fixture::new(rate);
    // The plan routes the source alongside a device-clocked input.
    fixture.route(ConsumerRoute::Routed { head: false });
    let mut graph = HardwareGraph::new(&fixture);
    for _ in 0..750 {
        fixture.write();
        graph.run(&fixture);
        fixture.cycle();
    }
    assert_eq!(graph.rewind, 0);
    fixture.request(false);
    graph.joined = true;
    fixture.period();
    let mut allocations = 0;
    for _ in 0..7_500 {
        fixture.write();
        graph.run(&fixture);
        allocations += allocation_counter::measure(|| fixture.cycle()).count_total;
    }
    assert_eq!(allocations, 0, "the sinc path allocated on the RT pump");
    assert!(graph.reads > 7_000, "the virtual source stayed silent");
    assert_eq!(graph.misses, 0);
    // The join may align both inputs to the virtual source's own reserve (about 5 ms),
    // never to the retained head two seconds back.
    let frame_nanos = 1_000_000_000 / u64::from(rate);
    assert!(
        graph.rewind <= 512 * frame_nanos,
        "the hardware input rewound {} ms",
        graph.rewind / 1_000_000
    );
    let latency = fixture.time(fixture.written) - graph.cursor.unwrap();
    assert!(
        latency <= 1_024 * frame_nanos,
        "the joined graph runs {} ms behind",
        latency / 1_000_000
    );
    let correction = fixture.pump.converter.as_ref().unwrap().correction_ppm();
    assert!(
        correction.abs() < 50.0,
        "drift correction pinned at {correction} ppm"
    );
}

/// A metered-only source keeps publishing at its live edge, so its meter and any pacing
/// on it follow the client, while it retains nothing and reports no loss. Routing it
/// later continues the same stream: nothing from before the route reaches the graph.
#[test]
fn an_unrouted_source_publishes_its_live_edge_and_a_later_route_continues_it() {
    let mut fixture = Fixture::new(48_000);
    // The plan meters the source without carrying it to an output.
    fixture.route(ConsumerRoute::Unrouted);
    fixture.request(true);
    let mut published = 0;
    // Three seconds, longer than a client queue holds.
    for _ in 0..1_125 {
        fixture.period();
        let (_, total, credit) = fixture.progress();
        assert!(fixture.pump.backlog.frames() <= FEED_PERIODS as u64 * 128);
        assert!(total > published, "publication froze while unrouted");
        assert_eq!(credit, total);
        published = total;
    }
    assert_eq!(published, fixture.written);
    // The meter reads the newest period, at its original coordinate.
    fixture.verify(fixture.written - 128);
    let epoch = fixture.progress().0;
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..16 {
        fixture.period();
    }
    assert_eq!(
        fixture.progress(),
        (epoch, fixture.written, fixture.written)
    );
    assert_eq!(fixture.pump.losses(), (0, 0));
    assert_eq!(
        fixture.pump.report.final_losses(fixture.pump.losses()),
        None
    );
}

#[test]
fn a_queue_overrun_before_admission_delivers_both_runs_to_the_graph() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..64 {
        fixture.period();
    }
    // The pump stalls while the client publishes past a whole queue.
    let gap = 1_024;
    let stalled = u64::from(QUEUE_FRAMES) + gap;
    for _ in 0..stalled / 128 {
        fixture.write();
    }
    fixture.cycle();
    assert_eq!(fixture.pump.mixer.skipped_frames(), gap);
    assert_eq!(
        fixture.pump.backlog.frames(),
        64 * 128 + u64::from(QUEUE_FRAMES)
    );
    fixture.request(true);
    // The graph reads two blocks a period: faster than live, slower than catch-up.
    let mut expected = (0..64 * 128)
        .step_by(128)
        .chain((64 * 128 + gap..).step_by(128));
    let mut next = expected.next().unwrap();
    let mut epochs = Vec::new();
    while next < fixture.written {
        assert!(fixture.written < 200_000, "the graph never caught up");
        fixture.period();
        let epoch = fixture.progress().0;
        if epochs.last() != Some(&epoch) {
            epochs.push(epoch);
        }
        for _ in 0..2 {
            let Ok(output) = fixture.read(next) else {
                break;
            };
            check(&output, next);
            next = expected.next().unwrap();
        }
    }
    // The run before the gap was read whole before the gap restarted the stream.
    assert_eq!(epochs.len(), 2, "{epochs:?}");
    assert!(next > 64 * 128 + gap, "the second run was never read");
    assert_eq!(fixture.pump.losses().0, gap);
}

#[test]
fn native_stop_ends_retention_while_the_graph_format_is_unset() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..375 {
        fixture.period();
    }
    assert_eq!(fixture.pump.backlog.frames(), 375 * 128);
    let anchor = fixture.anchor;
    assert!(
        fixture
            .writer()
            .set_clock((2, anchor), 48_000, false, false)
    );
    fixture.cycle();
    assert_eq!(fixture.pump.backlog.frames(), 0);
    assert_eq!(
        fixture.pump.report.final_losses((0, 0)).unwrap().stopped,
        375 * 128
    );
    // The next native timeline is retained from its own start, without the stopped one.
    fixture.anchor += fixture.timebase.ticks(Duration::from_secs(10));
    let anchor = fixture.anchor;
    assert!(fixture.writer().set_clock((3, anchor), 48_000, true, true));
    fixture.written = 0;
    for _ in 0..16 {
        fixture.period();
    }
    fixture.request(true);
    for _ in 0..8 {
        fixture.period();
    }
    assert_eq!(fixture.progress().1, fixture.written);
    for first in (0..fixture.written).step_by(128) {
        fixture.verify(first);
    }
    // A running source stops at once, and stays stopped with its output still pending.
    assert!(
        fixture
            .writer()
            .set_clock((3, anchor), 48_000, false, false)
    );
    fixture.cycle();
    let stopped = fixture.registry.clock(source_token()).unwrap();
    assert_eq!(stopped.running, 0);
    // The client leaves; its stopped timeline's converted output must never be written.
    fixture.writer = None;
    fixture.pump.converter.as_mut().unwrap().hold_output(128);
    for _ in 0..4 {
        fixture.cycle();
    }
    let after = fixture.registry.clock(source_token()).unwrap();
    assert_eq!(
        (after.running, after.native_frames),
        (0, stopped.native_frames)
    );
}

/// HAL grants can create the source before the graph's next cycle publishes its route.
/// Until then the pump keeps what a client queue holds, without converting it, and a
/// plan that already routed the source to its head consumer receives it from the
/// client's first frame.
#[test]
fn a_source_created_before_its_route_is_published_keeps_the_head_in_its_queue() {
    let mut fixture = Fixture::new(48_000);
    for _ in 0..32 {
        fixture.period();
    }
    assert_eq!(fixture.pump.backlog.frames(), 32 * 128);
    assert_eq!(fixture.progress(), (0, 0, 0));
    fixture.route(ConsumerRoute::Routed { head: true });
    fixture.request(true);
    for _ in 0..16 {
        fixture.period();
    }
    assert_eq!(fixture.progress().1, fixture.written);
    for first in (0..fixture.written).step_by(128) {
        fixture.verify(first);
    }
}

/// A routed source the pacer did not await, such as one it chose while already running,
/// joins at its live edge even as clock master: the pacer starts it at current credit,
/// so a retained head would only rewind the graph.
#[test]
fn a_routed_source_that_is_not_the_head_joins_at_the_live_edge() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: false });
    for _ in 0..375 {
        fixture.period();
    }
    assert_eq!(fixture.pump.backlog.frames(), 375 * 128);
    fixture.request(true);
    fixture.period();
    let (_, published, _) = fixture.progress();
    assert!(
        published <= 256,
        "{published} retained frames reached the source"
    );
    assert!(fixture.read(0).is_err(), "the retained head was published");
    assert_eq!(fixture.pump.losses(), (0, 0));
}

/// A route that stays unpublished for longer than a client queue holds (a format-gated
/// or direct-rendered worker) keeps one queue of PCM, and reports no loss for the rest.
#[test]
fn a_long_unpublished_route_keeps_one_queue_without_reporting_loss() {
    let mut fixture = Fixture::new(48_000);
    for _ in 0..1_125 {
        fixture.period();
    }
    assert_eq!(fixture.pump.backlog.frames(), u64::from(QUEUE_FRAMES));
    fixture.route(ConsumerRoute::Routed { head: true });
    fixture.request(true);
    for _ in 0..128 {
        fixture.period();
    }
    let first = fixture.written - 128 * 128 - u64::from(QUEUE_FRAMES);
    assert_eq!(fixture.progress().1, fixture.written - first);
    fixture.verify(first);
    assert_eq!(fixture.pump.losses(), (0, 0));
}

/// Conversion storage that cannot be prepared delays admission with a bounded backoff
/// instead of ending the endpoint, and the retained head still arrives whole.
#[test]
fn a_failed_converter_preparation_retries_and_keeps_the_head() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..64 {
        fixture.period();
    }
    fixture.pump.failing_converters = 3;
    fixture.request(true);
    let epoch = fixture.pump.transport.clock().unwrap().epoch;
    // 10 + 20 + 40 ms of backoff, then admission.
    let mut cycles = 0;
    while fixture.progress().1 == 0 {
        assert!(cycles < 64, "admission never retried");
        // Failed preparation comes before the epoch: the source keeps its stream.
        assert_eq!(fixture.pump.transport.clock().unwrap().epoch, epoch);
        fixture.period();
        cycles += 1;
    }
    assert!(
        cycles >= 26,
        "retried after {cycles} periods, inside the backoff"
    );
    for _ in 0..64 {
        fixture.period();
    }
    assert_eq!(fixture.progress().1, fixture.written);
    for first in (0..fixture.written).step_by(128) {
        fixture.verify(first);
    }
}

/// Gated rendering leaves a routed source filling its retention for seconds: the loss
/// episode is reported once, after the losses stop, not on every cycle.
#[test]
fn a_loss_episode_is_reported_once() {
    let timebase = Timebase::current();
    let mut report = Report::new(0);
    let period = timebase.ticks(Duration::from_nanos(2_666_667));
    let mut reports = Vec::new();
    // Five seconds of growing losses, then two quiet seconds.
    for cycle in 0..2_625_u64 {
        let dropped = cycle.min(1_875) * 128;
        if let Some(episode) = report.due_losses(cycle * period, timebase, (0, dropped)) {
            reports.push((cycle, episode.dropped));
        }
    }
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].1, 1_875 * 128);
    assert!(reports[0].0 > 1_875, "reported before the episode ended");
}

/// A configuration the converter can never accept ends the endpoint instead of retrying.
#[test]
fn an_invalid_converter_configuration_ends_the_endpoint() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    fixture.period();
    fixture.pump.failing_converters = 1;
    fixture.pump.converter_failure = mixrelay_audio_io::clock_bridge::BridgeError::InvalidConfig;
    fixture.request(true);
    fixture.write();
    let nanos = (u128::from(fixture.written) + 32) * 1_000_000_000 / 48_000;
    let now = fixture.anchor
        + fixture
            .timebase
            .ticks(Duration::from_nanos(u64::try_from(nanos).unwrap()));
    assert!(matches!(
        fixture
            .pump
            .cycle(now, fixture.timebase, &fixture.scheduling),
        Err(Error::Bridge(
            mixrelay_audio_io::clock_bridge::BridgeError::InvalidConfig
        ))
    ));
}

/// A client that plays and stops before any route is published, or while the plan
/// only meters it, loses nothing anyone wanted, so no loss is reported.
#[test]
fn stopping_an_unpublished_or_unrouted_source_reports_no_loss() {
    for route in [None, Some(ConsumerRoute::Unrouted)] {
        let mut fixture = Fixture::new(48_000);
        if let Some(route) = route {
            fixture.route(route);
        }
        for _ in 0..1_000 {
            fixture.period();
        }
        assert!(fixture.pump.backlog.frames() != 0);
        let anchor = fixture.anchor;
        assert!(
            fixture
                .writer()
                .set_clock((2, anchor), 48_000, false, false)
        );
        fixture.cycle();
        assert_eq!(fixture.pump.backlog.frames(), 0);
        assert_eq!(
            fixture.pump.report.final_losses(fixture.pump.losses()),
            None,
            "{route:?}"
        );
    }
}

/// The pacer can stop awaiting a source after its head stream began, before the graph
/// republishes the route. The pump re-checks the route each cycle, so the rest of the
/// head joins at the live edge rather than reaching a graph that will not consume it.
#[test]
fn a_head_stream_whose_route_loses_the_head_joins_the_live_edge() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..375 {
        fixture.period();
    }
    fixture.request(true);
    fixture.period();
    let (head_epoch, published, _) = fixture.progress();
    assert_eq!(published, FEED_PERIODS as u64 * 128);
    fixture.route(ConsumerRoute::Routed { head: false });
    fixture.period();
    assert!(fixture.pump.backlog.frames() <= 128);
    let (epoch, published, _) = fixture.progress();
    assert_ne!(epoch, head_epoch, "the live edge starts a new stream");
    assert!(
        published <= 256,
        "{published} frames of the old head followed"
    );
}

#[test]
fn an_idle_cached_endpoint_reports_its_unpublished_loss_once_after_the_quiet_period() {
    let mut fixture = Fixture::new(48_000);
    fixture.route(ConsumerRoute::Routed { head: true });
    for _ in 0..8 {
        fixture.period();
    }
    *fixture.pump.signals.update.lock() = Some(super::super::Update {
        queues: Vec::new(),
        received: 0,
        end_retention: true,
    });
    fixture.pump.update();
    let losses = fixture.pump.losses();
    assert_eq!(
        fixture.pump.report.final_losses(losses).unwrap().stopped,
        1_024
    );
    let now = fixture.anchor + fixture.timebase.ticks(Duration::from_secs(1));
    assert_eq!(
        fixture.pump.idle_wait(now, fixture.timebase),
        Some(Duration::from_secs(1))
    );
    let quiet = now + fixture.timebase.ticks(Duration::from_secs(1));
    assert_eq!(fixture.pump.idle_wait(quiet, fixture.timebase), None);
    assert!(fixture.pump.report.final_losses(losses).is_none());
    assert_eq!(fixture.pump.idle_wait(quiet, fixture.timebase), None);
}
