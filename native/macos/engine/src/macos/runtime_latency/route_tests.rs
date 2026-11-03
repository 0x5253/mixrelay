//! The published recording latency of a cross-clock route against the delay audio
//! actually takes through it, in process: no `CoreAudio` device and no installed build.
use super::{Measurements, threshold};
use crate::macos::{
    MacEngineExternalIo, MacTimeline, NativePacers, RouteClock, WorkerCycle,
    direct::render_directly, next_native_pace, process_native_cycle,
    worker_runtime::HARDWARE_CLOCK,
};
use crate::macos_devices::{
    DeviceSet, MacDevices,
    callback_fixture::{CallbackTime, NativeOwner, direct_plan, publish},
};
use mixrelay_engine_core::session::AdapterRuntimeStatus;
use mixrelay_render::{BlockProcessor, PeakMeterBank, RenderPlan};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

mod application;

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// The capture device runs fast against host time.
const SOURCE_PPM: f64 = 300.0;
/// Callback arrival after the last captured frame, and lead before the first presented one.
const CAPTURE_SAFETY_NS: f64 = 700_000.0;
const PRESENTATION_SAFETY_NS: f64 = 1_300_000.0;
const START_NS: f64 = 1e9;
/// Not a multiple of either callback, so impulses sample many buffer positions.
const IMPULSE_PERIOD: u64 = RATE as u64 / 4 + 99;
const IMPULSE_FRAMES: u64 = 32;
const IMPULSE_LEVEL: f32 = 0.5;
/// The reporter's production tick.
const WINDOW_NS: f64 = 1e9;
/// Converters settle first. A frame's delay then depends on where the two callback
/// clocks are relative to each other, which drift sweeps once per callback beat
/// (512 frames / (`RATE` * `SOURCE_PPM`) = 35.6 s): compare means over two whole beats.
const SETTLE_NS: f64 = 36e9;
const MEASURE_NS: f64 = 72e9;

/// One device's native callback clock against host time. Callbacks cycle through
/// `quanta`; capture stays continuous whatever each callback's size.
struct Clock<'a> {
    rate: f64,
    quanta: &'a [usize],
    input: bool,
    callbacks: usize,
    /// Frames of all earlier callbacks.
    position: u64,
}

#[allow(clippy::cast_precision_loss)]
impl Clock<'_> {
    /// The next callback's frame count.
    fn frames(&self) -> usize {
        self.quanta[self.callbacks % self.quanta.len()]
    }
    fn ns(&self, frames: u64) -> f64 {
        frames as f64 * 1e9 / self.rate
    }
    /// The next callback's arrival. Input arrives once its whole buffer was captured.
    fn now_ns(&self) -> f64 {
        if self.input {
            START_NS + self.ns(self.position + self.frames() as u64) + CAPTURE_SAFETY_NS
        } else {
            START_NS + self.ns(self.position)
        }
    }
    /// Capture or presentation time of the next callback's first frame.
    fn first_ns(&self) -> f64 {
        if self.input {
            START_NS + self.ns(self.position)
        } else {
            self.now_ns() + self.ns(self.frames() as u64) + PRESENTATION_SAFETY_NS
        }
    }
    fn time(&self, ticks_per_ns: f64) -> CallbackTime {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ticks = |ns: f64| (ns * ticks_per_ns).round() as u64;
        CallbackTime {
            now: ticks(self.now_ns()),
            host: ticks(self.first_ns()),
            sample: self.position as f64,
            rate_scalar: f64::from(RATE) / self.rate,
        }
    }
    fn advance(&mut self) {
        self.position += self.frames() as u64;
        self.callbacks += 1;
    }
}

/// The graph worker's device-paced block loop, as `process_worker_block` runs it.
struct Worker<'a> {
    sources: Option<&'a mixrelay_macos_transport::source_registry::SourceRegistry>,
    processor: BlockProcessor,
    pacers: NativePacers,
    timeline: MacTimeline,
    status: AdapterRuntimeStatus,
}

impl Worker<'_> {
    /// Renders every graph block the pacing device's credit admits.
    fn render(&mut self, devices: &DeviceSet) {
        loop {
            let route = RouteClock::observe(&self.processor, devices);
            let pacing = next_native_pace(&mut self.pacers, devices, self.processor.plan(), 16_384);
            assert_eq!(
                pacing.1, HARDWARE_CLOCK,
                "the endpoint clock paces the graph"
            );
            let Some((frames, discontinuity)) = self.timeline.accept(pacing, &route, &self.status)
            else {
                return;
            };
            let mut io = MacEngineExternalIo::new(devices);
            io.sources = self.sources;
            io.alignment = std::mem::take(&mut self.timeline.capture_alignment);
            let report = process_native_cycle(
                &mut self.processor,
                &mut io,
                WorkerCycle {
                    timeline: &self.timeline,
                    frames,
                    discontinuity: discontinuity || self.timeline.discontinuity_pending,
                },
            )
            .unwrap();
            self.timeline.capture_alignment = io.alignment;
            devices.record_latency_cycle(&self.processor, report, self.timeline.epoch);
            super::record_sources(
                &self.processor,
                self.sources,
                report.is_some(),
                self.timeline.epoch,
            );
            self.timeline.discontinuity_pending = report.is_none();
            if let Some(report) = report {
                self.status.record_process(report);
            }
            self.timeline.frame += u64::from(frames);
        }
    }
}

/// A capture device feeding a virtual endpoint's render stream (`to_os`).
fn capture_to_endpoint() -> BlockProcessor {
    route_processor(false)
}

fn route_processor(application: bool) -> BlockProcessor {
    use mixrelay_audio_graph::{
        CompileContext, CompileLimits, GraphCompiler, GraphDocument, NodeConfig, RenderFormat,
    };
    let mut document: GraphDocument = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,"nodes":[
            {"id":"input","type":"audio_device_input","config":{"selector":{"match":"default_input"},"outputChannels":2,"gainDb":0}},
            {"id":"virtual","type":"virtual_endpoint",
                "config":{"id":"01010101-0101-0101-0101-010101010101","displayName":"Fixture",
                    "reference":{"render":false,"capture":true},
                    "render":{"channels":2,"channelLayout":"stereo"},"capture":null,
                    "supportedSampleRates":[48000],"defaultSampleRate":48000}}
        ],"edges":[
            {"id":"record","source":{"nodeId":"input","portId":"audio"},"destination":{"nodeId":"virtual","portId":"to_os"}}
        ]
    }))
    .unwrap();
    if application {
        document.nodes[0].config = NodeConfig::ApplicationCapture(
            serde_json::from_value(serde_json::json!({
                "selector":{"match":"bundle_id","bundle_id":"fixture","include_descendants":false},
                "stream":{"channels":2,"channelLayout":"stereo"},"sourcePlayback":"preserve"
            }))
            .unwrap(),
        );
    }
    if let NodeConfig::VirtualEndpoint(endpoint) = &document.nodes[1].config {
        document.nodes[1].config = NodeConfig::VirtualEndpointTransport(endpoint.spec.clone());
    }
    let execution = GraphCompiler::default()
        .compile(
            &document,
            CompileContext {
                format: RenderFormat {
                    sample_rate: RATE,
                    block_frames: 32,
                },
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    BlockProcessor::prepare_with_hosted_processors(
        &execution,
        Arc::new(PeakMeterBank::new(8, 64)),
        &crate::macos_devices::latency::tests::Factory,
    )
    .unwrap()
}

/// Source PCM: silence and a step of `IMPULSE_FRAMES` every `IMPULSE_PERIOD`, after the first.
fn fill_source(pcm: &mut [f32], first_frame: u64) {
    for (frame, samples) in (first_frame..).zip(pcm.chunks_exact_mut(usize::from(CHANNELS))) {
        let on = frame >= IMPULSE_PERIOD && frame % IMPULSE_PERIOD < IMPULSE_FRAMES;
        samples.fill(if on { IMPULSE_LEVEL } else { 0.0 });
    }
}

/// Host times at which the presented signal rises through half the impulse level.
#[derive(Default)]
struct Arrivals {
    previous: Option<(f32, f64)>,
    crossings: Vec<f64>,
}

impl Arrivals {
    #[allow(clippy::cast_precision_loss)]
    fn observe(&mut self, pcm: &[f32], first_ns: f64) {
        let half = IMPULSE_LEVEL / 2.0;
        for (index, samples) in pcm.chunks_exact(usize::from(CHANNELS)).enumerate() {
            let (value, time) = (samples[0], first_ns + index as f64 * 1e9 / f64::from(RATE));
            if let Some((before, at)) = self.previous
                && before < half
                && value >= half
            {
                let fraction = f64::from((half - before) / (value - before));
                self.crossings.push(at + fraction * (time - at));
            }
            self.previous = Some((value, time));
        }
    }
}

/// Host time at which the source captured impulse `index` (from zero).
#[allow(clippy::cast_precision_loss)]
fn impulse_capture_ns(source: &Clock<'_>, index: usize) -> f64 {
    START_NS + ((index as u64 + 1) * IMPULSE_PERIOD) as f64 * 1e9 / source.rate
}

/// The reporter's tick: one window of the active plan's observations.
fn report(devices: &MacDevices, measurements: &mut Measurements, plan: &RenderPlan) -> Option<u32> {
    let id = plan.sink_bindings()[0]
        .virtual_endpoint_id()
        .unwrap()
        .clone();
    let rates = BTreeMap::from([(id.clone(), RATE)]);
    let observations = devices.latency_observations(plan.revision(), false);
    measurements.observe(plan, plan.revision(), observations, &rates)[&id]
}

/// Where the graph renders: a worker paced by the endpoint, or inside one device's
/// callback while the other device keeps its converter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rendering {
    Worker,
    DirectSource,
    DirectSink,
}

#[derive(Clone, Copy)]
struct Route {
    source_frames: &'static [usize],
    sink_frames: usize,
    rendering: Rendering,
}

/// A capture device alternating small and large callbacks, 512 frames per pair so the
/// drift beat matches the fixed routes. Counting each callback once, its capture
/// interval would mean 256 frames instead of the 400 its frames actually wait.
const VARIABLE_SOURCE: &[usize] = &[64, 448];

#[test]
fn bridged_capture_route_reports_the_delay_its_impulse_actually_takes() {
    assert_reports(Route {
        source_frames: &[256],
        sink_frames: 512,
        rendering: Rendering::Worker,
    });
}

/// A direct endpoint fixture isolates input-bridge measurement from worker pacing.
#[test]
fn bridged_capture_into_a_direct_endpoint_reports_its_actual_delay() {
    assert_reports(Route {
        source_frames: &[256],
        sink_frames: 512,
        rendering: Rendering::DirectSink,
    });
}

#[test]
fn direct_capture_into_a_bridged_endpoint_reports_its_actual_delay() {
    assert_reports(Route {
        source_frames: &[512],
        sink_frames: 256,
        rendering: Rendering::DirectSource,
    });
}

#[test]
fn bridged_capture_with_variable_callbacks_reports_its_actual_delay() {
    assert_reports(Route {
        source_frames: VARIABLE_SOURCE,
        sink_frames: 512,
        rendering: Rendering::Worker,
    });
}

fn assert_reports(route: Route) {
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::macos::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let devices = MacDevices::without_devices(Arc::clone(&status), RATE, 32);
    let source = NativeOwner::new(CHANNELS, RATE, true);
    let sink = NativeOwner::new(CHANNELS, RATE, false);
    let active = publish(&devices, &[("input", &source), ("virtual/render", &sink)]);
    let owners = (&source, &sink);
    let Outcome { published, delays } = if route.rendering == Rendering::Worker {
        let mut worker = Worker {
            sources: None,
            processor: capture_to_endpoint(),
            pacers: NativePacers::default(),
            timeline: MacTimeline::new(),
            status: AdapterRuntimeStatus::new(
                crate::macos::adapter_profile(),
                std::sync::Arc::new(mixrelay_time::ManualClock::default()),
            ),
        };
        let plan = Arc::clone(worker.processor.plan());
        // The first pacing pass selects the endpoint as clock master; each device worker then
        // publishes its converter for that alignment, as the connected supervisor does.
        worker.render(&active);
        let _served = (source.serve(), sink.serve());
        run(&devices, &plan, route, owners, || worker.render(&active))
    } else {
        let ((node, key, master), (bridged_key, bridged)) =
            if route.rendering == Rendering::DirectSink {
                (("virtual", "virtual/render", &sink), ("input", &source))
            } else {
                (("input", "input", &source), ("virtual/render", &sink))
            };
        let plan = direct_plan(
            node,
            key,
            master,
            publish(&devices, &[(bridged_key, bridged)]),
        );
        let mut processor = Box::new(capture_to_endpoint());
        let graph = Arc::clone(processor.plan());
        // Installation unaligns the bridged clock before its converter is published.
        render_directly(&plan, &mut processor, || {
            let _served = bridged.serve();
            run(&devices, &graph, route, owners, || {})
        })
    };
    let span = START_NS + SETTLE_NS..START_NS + SETTLE_NS + MEASURE_NS;
    let reported = published
        .iter()
        .filter(|(end, _)| span.contains(&(end - WINDOW_NS)))
        .map(|(end, value)| {
            let value = value.unwrap_or_else(|| panic!("published at {:.0} s", end / 1e9));
            f64::from(value)
        })
        .collect::<Vec<_>>();
    let actual = delays
        .iter()
        .filter(|(captured, _)| span.contains(captured))
        .map(|(_, delay)| delay * f64::from(RATE) / 1e9)
        .collect::<Vec<_>>();
    let (reported, actual) = (Spread::of(&reported), Spread::of(&actual));
    eprintln!(
        "capture {:?} frames +{SOURCE_PPM} ppm -> endpoint {} frames, {}: reported {reported}, \
         actual {actual} frames",
        route.source_frames,
        route.sink_frames,
        match route.rendering {
            Rendering::Worker => "bridged",
            Rendering::DirectSource => "direct capture",
            Rendering::DirectSink => "direct endpoint",
        }
    );
    assert!(
        (reported.mean - actual.mean).abs() <= f64::from(threshold(RATE)),
        "reported {reported}, actual {actual} frames"
    );
}

/// Mean and range of per-window or per-impulse frame counts.
struct Spread {
    mean: f64,
    minimum: f64,
    maximum: f64,
}

impl Spread {
    #[allow(clippy::cast_precision_loss)]
    fn of(values: &[f64]) -> Self {
        assert!(values.len() >= 2, "at least two measurements");
        Self {
            mean: values.iter().sum::<f64>() / values.len() as f64,
            minimum: values.iter().copied().fold(f64::INFINITY, f64::min),
            maximum: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

impl std::fmt::Display for Spread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "mean {:.1} [{:.1}, {:.1}]",
            self.mean, self.minimum, self.maximum
        )
    }
}

/// Host times in ns.
struct Outcome {
    /// Each reporter window's end and published value.
    published: Vec<(f64, Option<u32>)>,
    /// Each impulse's capture time and capture-to-presentation delay.
    delays: Vec<(f64, f64)>,
}

/// Drives both callback clocks in host-time order with `render` after each endpoint
/// callback (a worker paced by it), and the reporter once per window.
#[allow(clippy::cast_precision_loss)]
fn run(
    devices: &MacDevices,
    plan: &RenderPlan,
    route: Route,
    (source, sink): (&NativeOwner, &NativeOwner),
    mut render: impl FnMut(),
) -> Outcome {
    let ticks_per_ns =
        mixrelay_realtime::Timebase::current().ticks(Duration::from_secs(1)) as f64 / 1e9;
    let mut clocks = [
        Clock {
            rate: f64::from(RATE) * (1.0 + SOURCE_PPM * 1e-6),
            quanta: route.source_frames,
            input: true,
            callbacks: 0,
            position: 0,
        },
        Clock {
            rate: f64::from(RATE),
            quanta: std::slice::from_ref(&route.sink_frames),
            input: false,
            callbacks: 0,
            position: 0,
        },
    ];
    let largest = route.source_frames.iter().copied().max().unwrap_or(0);
    let mut capture = vec![0.0; largest * usize::from(CHANNELS)];
    let mut presentation = vec![0.0; route.sink_frames * usize::from(CHANNELS)];
    let (mut arrival, mut measurements) = (Arrivals::default(), Measurements::default());
    let (mut window, mut published) = (START_NS + WINDOW_NS, Vec::new());
    while clocks[1].now_ns() < START_NS + SETTLE_NS + MEASURE_NS + WINDOW_NS {
        let next = usize::from(clocks[1].now_ns() < clocks[0].now_ns());
        let clock = &mut clocks[next];
        if clock.now_ns() >= window {
            published.push((window, report(devices, &mut measurements, plan)));
            window += WINDOW_NS;
        }
        if clock.input {
            let pcm = &mut capture[..clock.frames() * usize::from(CHANNELS)];
            fill_source(pcm, clock.position);
            source.callback(pcm, clock.time(ticks_per_ns));
        } else {
            sink.callback(&mut presentation, clock.time(ticks_per_ns));
            arrival.observe(&presentation, clock.first_ns());
            render();
        }
        clock.advance();
    }
    let delays = arrival
        .crossings
        .iter()
        .enumerate()
        .map(|(index, presented)| {
            let captured = impulse_capture_ns(&clocks[0], index);
            (captured, presented - captured)
        })
        .collect();
    Outcome { published, delays }
}
