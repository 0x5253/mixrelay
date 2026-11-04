//! Native device assertions shared by the application controller fixture.
#![cfg_attr(
    any(test, feature = "test-util"),
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "Owned application fixtures fail immediately on invalid setup"
    )
)]
use super::ffi::{mr_device_io_overflows, mr_device_io_ticks};
use super::{
    Arc, AtomicBool, BTreeMap, Device, DeviceSet, Duration, Instant, MacDevices, Ordering, Spec,
    Weak, simulated, spec::specs,
};
#[cfg(test)]
use super::{aggregate, device_fixture, direct, ffi, worker_retirement};
#[cfg(test)]
use super::{ffi::mr_device_io_quantum, native::Native};
use mixrelay_audio_graph::GraphDocument;
#[cfg(test)]
use mixrelay_audio_graph::RenderFormat;
#[cfg(test)]
use mixrelay_audio_graph::{
    AudioDeviceInputConfig, AudioDeviceOutputConfig, DeviceSelector, NodeConfig, NodeSpec,
};
use mixrelay_audio_io::process::{InputActivity, TransferOutcome};
#[cfg(test)]
use mixrelay_engine_core::{
    GraphResourceBackend, PlaybackBindings::Preserve, session::AdapterRuntimeStatus,
};
use mixrelay_render::{BlockProcessor, ProcessorExchange};
#[cfg(test)]
use simulated::DEFAULT_BUFFER;
use simulated::{CAPTURED, SimulatedHost};
use std::{collections::BTreeSet, thread};
/// Simulated native frames the hardware-paced graph must transfer through one format.
const DRIVE_FRAMES: u64 = 4096;
/// Every driving cycle advances the pacing clock by at least the smallest session buffer,
/// so more cycles than this mean the native clock stalled.
const CYCLE_BUDGET: u64 = DRIVE_FRAMES / 32 + 2;
/// The level the graph renders to the output device.
#[cfg(test)]
const RENDERED: f32 = 0.5;
/// Converted DC may differ from its source by the converter's rounding only.
const LEVEL_TOLERANCE: f32 = 1e-3;
/// Frames at the end of a drive whose level is checked, after conversion settled.
const SETTLED_FRAMES: usize = 128;

/// Waits for device workers. Only a stalled worker reaches the guard; the outcome itself
/// is decided by the observed state, never by elapsed time.
pub(super) fn wait_until(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "native device transition stalled"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
fn simulated(rate: u32) -> (Arc<SimulatedHost>, String, MacDevices) {
    let host = SimulatedHost::new();
    let uid = host.plug("io.mixrelay.test.duplex", 2, 48_000, CAPTURED);
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::test_support::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let resources = MacDevices::with_host(status, rate, 256, host.clone());
    (host, uid, resources)
}

#[cfg(test)]
fn format(sample_rate: u32, block_frames: u32) -> RenderFormat {
    RenderFormat {
        sample_rate,
        block_frames,
    }
}

#[cfg(test)]
fn document(uid: &str, revision: u64, gain: f32) -> GraphDocument {
    duplex_document(uid, uid, revision, gain)
}

#[cfg(test)]
fn duplex_document(microphone: &str, speaker: &str, revision: u64, gain: f32) -> GraphDocument {
    GraphDocument::new(
        revision,
        vec![
            NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: "input".into(),
                label: None,
                config: NodeConfig::AudioDeviceInput(AudioDeviceInputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: microphone.into(),
                    },
                    output_channels: 2,
                    gain_db: gain,
                }),
            },
            NodeSpec {
                control: mixrelay_audio_graph::NodeControl::default(),
                id: "output".into(),
                label: None,
                config: NodeConfig::AudioDeviceOutput(AudioDeviceOutputConfig {
                    selector: DeviceSelector::PersistentUid {
                        uid: speaker.into(),
                    },
                    input_channels: 2,
                }),
            },
        ],
        vec![],
    )
}

/// `graph` with its input wired to its output.
#[cfg(test)]
fn wired(graph: &GraphDocument) -> GraphDocument {
    let mut wired = serde_json::to_value(graph).unwrap();
    wired["edges"] = serde_json::json!([{
        "id": "wire",
        "source": {"nodeId": "input", "portId": "audio"},
        "destination": {"nodeId": "output", "portId": "audio"}
    }]);
    serde_json::from_value(wired).unwrap()
}

/// The compiled render plan at `rate` of `graph` as published.
#[cfg(test)]
fn compiled(graph: &GraphDocument, rate: u32) -> mixrelay_render::RenderPlan {
    use mixrelay_audio_graph::{CompileContext, CompileLimits, GraphCompiler};
    let execution = GraphCompiler::default()
        .compile(
            graph,
            CompileContext {
                format: format(rate, 256),
                limits: CompileLimits::default(),
            },
        )
        .unwrap();
    mixrelay_render::RenderPlan::prepare(&execution).unwrap()
}

/// The compiled render plan at `rate` of the graph with its input wired to its output.
#[cfg(test)]
fn render_plan(graph: &GraphDocument, rate: u32) -> mixrelay_render::RenderPlan {
    compiled(&wired(graph), rate)
}

#[cfg(test)]
fn activate(resources: &MacDevices, active: &mut DeviceSet) {
    resources.activation().unwrap().activate(active);
    resources.reclaim();
}

/// The format monitor's recovery pass, then the audio worker's recovery boundary.
fn publish_recovery_now(resources: &MacDevices, active: &mut DeviceSet) {
    *resources.next_group_check.lock().unwrap() = Instant::now();
    resources.recover(&AtomicBool::new(false));
    resources.activation().unwrap().activate_recovered(active);
    resources.reclaim();
}

fn wait_failed(active: &DeviceSet) {
    wait_until(|| {
        active
            .0
            .values()
            .all(|device| device.failed.load(Ordering::Acquire))
    });
}

/// The key of the active set's one native owner or offline selection per direction,
/// as `[input, output]`.
fn bindings(active: &DeviceSet) -> [String; 2] {
    [true, false].map(|input| {
        let owners = active.0.iter().filter(|(_, device)| device.input == input);
        let offline = active.1.iter().filter(|(_, spec)| spec.input == input);
        let mut ids = owners.map(|(id, _)| id).chain(offline.map(|(id, _)| id));
        match (ids.next(), ids.next()) {
            (Some(id), None) => id.clone(),
            _ => panic!("expected one binding with input={input}"),
        }
    })
}

/// Selected devices by direction, independent of the ids the controller compiled.
fn by_direction<'a>(specs: impl IntoIterator<Item = &'a Spec>) -> BTreeMap<bool, Spec> {
    specs
        .into_iter()
        .map(|spec| (spec.input, spec.clone()))
        .collect()
}

/// What one lockstep drive moved: graph input as captured, and output as the callbacks presented.
#[derive(Default)]
struct Driven {
    captured: Vec<f32>,
    presented: Vec<f32>,
}

/// One 256-frame graph block: the stereo PCM read from the input binding, to the stereo
/// PCM written to the output binding.
type Graph<'a> = &'a mut dyn FnMut(&[f32], &mut [f32]);

/// Runs the hardware and `graph` in lockstep. Each cycle runs one input and one output
/// callback, then exactly the graph blocks the pacing clock's native credit admits, until
/// `frames` native frames passed. Workers run no callback meanwhile, so the outcome is
/// independent of thread scheduling.
fn drive(
    host: &SimulatedHost,
    active: &DeviceSet,
    rate: u32,
    frames: u64,
    graph: Graph<'_>,
) -> Driven {
    let _held = host.hold_callbacks();
    let [input, output] = bindings(active);
    let ticks = || {
        // SAFETY: the active set retains both owners; the counters are atomic snapshots.
        [&input, &output]
            .map(|id| unsafe { mr_device_io_ticks(active.0[id].native.pointer.as_ptr()) })
    };
    let (callbacks, polls) = (ticks(), host.polls());
    let start = active.hardware_clock_progress().unwrap().native_frames;
    let (mut graph_frames, mut driven) = (0, Driven::default());
    let (mut pcm, mut rendered) = ([0.0; 512], [0.0; 512]);
    for cycle in 1..=CYCLE_BUDGET {
        host.callback(&active.0[&input].native)
            .expect("input registration started");
        let presented = host
            .callback(&active.0[&output].native)
            .expect("output registration started");
        driven.presented.extend(presented);
        let clock = active.hardware_clock_progress().unwrap();
        let native = clock.native_frames - start;
        let through = native * u64::from(rate) / u64::from(clock.native_rate);
        while graph_frames + 256 <= through {
            let read = active.read(&input, &mut pcm, 0).unwrap();
            assert_ne!(
                read.activity,
                InputActivity::Inactive,
                "input fell back offline"
            );
            driven
                .captured
                .extend_from_slice(&pcm[..read.frames as usize * 2]);
            graph(&pcm, &mut rendered);
            let written = active.write(&output, &rendered, 0).unwrap();
            assert_eq!(
                written,
                TransferOutcome::complete(256),
                "output fell back offline or dropped graph PCM"
            );
            graph_frames += 256;
        }
        if native >= frames {
            // Workers kept polling progress, yet only the test's steps ran callbacks.
            wait_until(|| host.polls() >= polls + 4);
            assert_eq!(
                ticks(),
                callbacks.map(|before| before + cycle),
                "a callback ran outside the test's steps"
            );
            return driven;
        }
    }
    panic!("native clock stalled within {CYCLE_BUDGET} cycles");
}

/// Every sample in the settled tail of interleaved stereo `pcm` has `level`.
fn assert_level(pcm: &[f32], level: f32, what: &str) {
    let tail = pcm.len().saturating_sub(SETTLED_FRAMES * 2);
    assert!(
        pcm.len() >= SETTLED_FRAMES * 2
            && pcm[tail..]
                .iter()
                .all(|sample| (sample - level).abs() <= LEVEL_TOLERANCE),
        "{what} does not carry {level}: {:?}",
        &pcm[tail..]
    );
}

/// Native capture reaches the graph as the device's signal, and the graph's output reaches
/// the native callback, with no packet dropped on either side.
#[cfg(test)]
fn assert_transfers(host: &SimulatedHost, active: &DeviceSet, rate: u32) {
    assert_presents(
        host,
        active,
        rate,
        &mut |_, output| output.fill(RENDERED),
        RENDERED,
    );
}

/// Like [`assert_transfers`], with `graph` rendering each block and the output callback
/// presenting `level`.
fn assert_presents(
    host: &SimulatedHost,
    active: &DeviceSet,
    rate: u32,
    graph: Graph<'_>,
    level: f32,
) {
    let overflows_before = overflows(active);
    let driven = drive(host, active, rate, DRIVE_FRAMES, graph);
    assert_level(&driven.captured, CAPTURED, "graph input");
    assert_level(&driven.presented, level, "output callback");
    assert_eq!(
        overflows(active),
        overflows_before,
        "native-paced graph must not drop input or output packets"
    );
}

#[test]
fn returning_group_member_reregisters_both_owners_on_a_new_private_group() {
    let host = SimulatedHost::new();
    let microphone = host.plug("io.mixrelay.test.microphone", 2, 48_000, CAPTURED);
    let speaker = host.plug("io.mixrelay.test.speaker", 2, 48_000, 2.0 * CAPTURED);
    let status = Arc::new(AdapterRuntimeStatus::new(
        crate::test_support::adapter_profile(),
        std::sync::Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let resources = MacDevices::with_host(status, 48_000, 256, host.clone());
    let graph = duplex_document(&microphone, &speaker, 1, 0.0);
    resources.reconcile(&graph, Preserve).unwrap();
    let mut active = DeviceSet::default();
    activate(&resources, &mut active);
    let [first] = host.groups()[..] else {
        panic!("two physical members need one private group");
    };
    host.unplug(&microphone);
    // The aggregate loses a member: both registrations on it fail, not only the microphone's.
    wait_failed(&active);
    assert_eq!(host.replug(&microphone), DEFAULT_BUFFER);
    // The returned member is a new object, so the old group no longer reads back as planned:
    // recovery plans a new group and registers both owners on it.
    wait_until(|| {
        publish_recovery_now(&resources, &mut active);
        active.0.len() == 2 && active.0.values().all(|device| device.ready())
    });
    let second = active.0["input"].clock_identity.load(Ordering::Acquire);
    assert_ne!(second, first, "owners stayed on the stale group");
    assert_eq!(
        active.0["output"].clock_identity.load(Ordering::Acquire),
        second
    );
    // One callback renders both members in place, each at its slice of the new group.
    let direct = direct::DirectPlan::prepare(&active, &render_plan(&graph, 48_000))
        .expect("one aggregate clock renders directly");
    assert_eq!(direct.clock_id, second);
    let slices = direct
        .bindings
        .iter()
        .map(|binding| (binding.id.as_str(), binding.input, binding.offset))
        .collect::<Vec<_>>();
    assert_eq!(
        slices,
        [("input", true, Some(2)), ("output", false, Some(0))]
    );
    drop(direct);
    // The output callback renders only the speaker's slice of the new group's stream.
    let driven = drive(&host, &active, 48_000, DRIVE_FRAMES, &mut |_, output| {
        output.fill(RENDERED);
    });
    assert_level(&driven.captured, CAPTURED, "graph input");
    let (speaker_slice, microphone_slice): (Vec<_>, Vec<_>) = driven
        .presented
        .chunks_exact(4)
        .flat_map(|frame| [(frame[0], frame[2]), (frame[1], frame[3])])
        .unzip();
    assert_level(&speaker_slice, RENDERED, "speaker slice");
    assert!(microphone_slice.iter().all(|sample| *sample == 0.0));
    resources
        .reconcile(&GraphDocument::new(2, vec![], vec![]), Preserve)
        .unwrap();
    activate(&resources, &mut active);
    wait_until(|| {
        worker_retirement::WORKERS.reclaim();
        aggregate::Aggregate::reclaim_retired();
        host.groups().is_empty()
    });
}

#[test]
fn final_owner_teardown_unregisters_through_its_host() {
    let host = SimulatedHost::new();
    let uid = host.plug("io.mixrelay.test.teardown", 2, 48_000, CAPTURED);
    let spec = Spec {
        selector: DeviceSelector::PersistentUid { uid },
        input: false,
        channels: 2,
    };
    let native = Native::create(&spec, 48_000, None, 0, host.clone()).unwrap();
    native.connect(&AtomicBool::new(false)).unwrap();
    assert_eq!(host.registrations(), 1);
    // Even without the connection guard's drain, the last owner unregisters before release.
    drop(native);
    assert_eq!(
        host.registrations(),
        0,
        "teardown bypassed the owner's host"
    );
}

#[test]
fn all_buffer_sizes_are_applied_and_survive_reconnection() {
    let (host, uid, resources) = simulated(48_000);
    let graph = document(&uid, 1, 0.0);
    resources.reconcile(&graph, Preserve).unwrap();
    let mut active = DeviceSet::default();
    activate(&resources, &mut active);
    assert_eq!(
        host.buffer(&uid),
        256,
        "initial connection applies the session block"
    );
    let mut previous = 256;
    for &frames in mixrelay_audio_io::formats::BUFFER_SIZES {
        let mut prepared = resources
            .prepare_format_change(
                &graph,
                Preserve,
                format(48_000, previous),
                format(48_000, frames),
            )
            .unwrap();
        prepared.configure_native().unwrap();
        prepared.commit();
        // The audio worker activates the committed set at its block boundary.
        activate(&resources, &mut active);
        assert_eq!(host.buffer(&uid), frames);
        let callbacks_use = |frames| {
            // SAFETY: the active set retains each owner; the quantum is an atomic snapshot.
            active.0.values().all(|device| unsafe {
                mr_device_io_quantum(device.native.pointer.as_ptr(), false) == frames
            })
        };
        wait_until(|| callbacks_use(frames));
        host.unplug(&uid);
        wait_failed(&active);
        // Observed as the device is republished, before any worker can reconnect.
        assert_eq!(
            host.replug(&uid),
            DEFAULT_BUFFER,
            "device returns with its default"
        );
        // Readiness needs a callback of the new connection, which runs after reapplication.
        wait_until(|| active.0.values().all(|device| device.ready()));
        assert_eq!(
            host.buffer(&uid),
            frames,
            "reconnection reapplies the session block"
        );
        wait_until(|| callbacks_use(frames));
        previous = frames;
    }
}

/// Native capture/output overflow counters of both directions.
fn overflows(active: &DeviceSet) -> [u64; 2] {
    // SAFETY: the active set retains both owners; the counters are atomic snapshots.
    bindings(active)
        .map(|id| unsafe { mr_device_io_overflows(active.0[&id].native.pointer.as_ptr()) })
}

#[cfg(test)]
fn native_rates(active: &DeviceSet) -> [u32; 2] {
    ["input", "output"].map(|id| active.0[id].native_rate.load(Ordering::Acquire))
}

#[test]
fn src_uses_actual_rate_and_rebuilds_graph_format_only() {
    let (host, uid, resources) = simulated(44_100);
    let graph = document(&uid, 1, 0.0);
    resources.reconcile(&graph, Preserve).unwrap();
    let mut active = DeviceSet::default();
    activate(&resources, &mut active);
    assert_eq!(
        native_rates(&active),
        [48_000; 2],
        "owners follow the device rate"
    );
    let before = Arc::as_ptr(&active.0["output"]);
    let mut old = format(44_100, 256);
    for rate in [44_100, 96_000, 48_000] {
        let next = format(rate, 256);
        // No concurrent graph processing here: commit and activation model the
        // acknowledged pause boundary. The host has no device-rate setter at all.
        if next != old {
            resources
                .prepare_format_change(&graph, Preserve, old, next)
                .unwrap()
                .commit();
            activate(&resources, &mut active);
        }
        wait_until(|| active.0.values().all(|device| device.ready()));
        let clock = active.hardware_clock_progress().unwrap();
        let before_master = ["input", "output"].map(|id| {
            let device = &active.0[id];
            (
                device.clock_aligned.load(Ordering::Acquire),
                device.epoch.load(Ordering::Acquire),
            )
        });
        active.set_clock_master(Some(clock.owner_id));
        // Every owner whose alignment changed rebuilds its bridge under a new epoch.
        wait_until(|| {
            ["input", "output"]
                .iter()
                .zip(before_master)
                .all(|(id, (aligned, epoch))| {
                    let device = &active.0[*id];
                    device.ready()
                        && (device.clock_aligned.load(Ordering::Acquire) == aligned
                            || device.epoch.load(Ordering::Acquire) != epoch)
                })
        });
        assert_transfers(&host, &active, rate);
        assert_eq!(active.clock_synchronized("input", true), Some(true));
        assert_eq!(active.clock_synchronized("output", false), Some(true));
        assert_eq!(Arc::as_ptr(&active.0["output"]), before);
        assert_eq!(native_rates(&active), [48_000; 2]);
        old = next;
    }
    drop(active);
    resources.reclaim();
}

#[test]
fn repeated_initial_connections_preserve_failure_diagnostics() {
    for iteration in 0..20 {
        let host = SimulatedHost::new();
        let uid = host.plug(
            &format!("io.mixrelay.test.initial-{iteration}"),
            2,
            48_000,
            CAPTURED,
        );
        let status = Arc::new(AdapterRuntimeStatus::new(
            crate::test_support::adapter_profile(),
            std::sync::Arc::new(mixrelay_time::ManualClock::default()),
        ));
        let rate = [44_100, 48_000, 96_000][iteration % 3];
        let resources = MacDevices::with_host(status, rate, 256, host.clone());
        resources
            .reconcile(&document(&uid, 1, 0.0), Preserve)
            .unwrap_or_else(|error| {
                panic!("initial connection iteration={iteration} graph_rate={rate}: {error}")
            });
        let mut active = DeviceSet::default();
        activate(&resources, &mut active);
        assert_eq!(active.0.len(), 2);
        assert!(active.0.values().all(|device| device.ready()));
    }
    // A rejected initial connection fails preparation with its native cause, after
    // restoring the client buffer it had already changed, and publishes nothing.
    let (host, uid, resources) = simulated(48_000);
    host.publish_unsupported_format(&uid);
    let error = resources
        .reconcile(&document(&uid, 1, 0.0), Preserve)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("native stream format or channel layout unavailable (-3)"),
        "{error}"
    );
    assert!(error.contains("buffer restored=true"), "{error}");
    assert_eq!(host.buffer(&uid), DEFAULT_BUFFER);
    assert!(resources.handoff.idle());
}

#[test]
fn recovered_input_reports_its_epoch_once_before_delivering_fresh_pcm() {
    let (device, context) = device_fixture::synthetic_device(1, 48_000, 48_000, true);
    let mut pcm = [9.0; 32];
    let initial = device.read(&mut pcm, 0);
    assert_eq!(initial.frames, 0);
    assert!(!initial.discontinuity);
    let measured = allocation_counter::measure(|| {
        // SAFETY: the synthetic owner has no OS registration. This test owns
        // both serialized queue roles and retains complete disjoint PCM buffers.
        unsafe {
            let pointer = device.native.pointer.as_ptr();
            assert_eq!(
                ffi::mr_device_io_bridge_write(pointer, [0.5; 32].as_ptr(), 32),
                32
            );
            let old = device.read(&mut pcm, 0);
            assert_eq!(old.frames, 32);
            assert!(!old.discontinuity);
            assert_eq!(
                ffi::mr_device_io_bridge_write(pointer, [0.5; 32].as_ptr(), 32),
                32
            );
            context.epoch.fetch_add(1, Ordering::Release);
            let recovered = device.read(&mut pcm, 0);
            assert_eq!(recovered.frames, 0);
            assert!(recovered.discontinuity);
            assert_eq!(pcm.map(f32::to_bits), [0; 32]);
            assert!(!device.read(&mut pcm, 0).discontinuity);
            assert_eq!(
                ffi::mr_device_io_bridge_write(pointer, [0.75; 32].as_ptr(), 32),
                32
            );
            let fresh = device.read(&mut pcm, 0);
            assert_eq!(fresh.frames, 32);
            assert!(!fresh.discontinuity);
            assert_eq!(pcm.map(f32::to_bits), [0.75_f32.to_bits(); 32]);
        }
    });
    assert_eq!(measured.count_total, 0);
}

/// Device owners retained by the application's simulated audio worker.
#[derive(Default)]
pub struct ActiveDevices(DeviceSet);

/// Weak observations do not keep retired native device owners alive.
pub struct OwnerLifetimes(Vec<Weak<Device>>);
impl OwnerLifetimes {
    #[must_use]
    pub fn all_released(&self) -> bool {
        self.0.iter().all(|owner| owner.upgrade().is_none())
    }
}

/// Retains the exact resource-consumer guard used at a native processing boundary.
pub struct Activation<'a>(super::DeviceActivation<'a>);
impl Activation<'_> {
    pub fn activate(&mut self, active: &mut ActiveDevices) {
        self.0.activate(&mut active.0);
    }
}
#[must_use]
/// # Panics
/// Panics if another resource consumer already owns the fixture.
pub fn activation(resources: &MacDevices) -> Activation<'_> {
    Activation(resources.activation().unwrap())
}
#[must_use]
pub fn has_pending(resources: &MacDevices) -> bool {
    resources.has_pending()
}
#[must_use]
/// # Panics
/// Panics if a previous fixture panic poisoned the selection cache.
pub fn cache_is_empty(resources: &MacDevices) -> bool {
    resources.cache.lock().unwrap().is_empty()
}
/// # Panics
/// Panics when the cached binding ids or selections differ from the graph.
pub fn assert_cached_selection(
    resources: &MacDevices,
    graph: &GraphDocument,
    bound: &BTreeSet<String>,
) {
    let cache = resources.cache.lock().unwrap();
    assert_eq!(cache.keys().cloned().collect::<BTreeSet<_>>(), *bound);
    assert_eq!(
        by_direction(cache.values().map(|(spec, _)| spec)),
        by_direction(specs(graph).values())
    );
}
impl ActiveDevices {
    /// # Panics
    /// Panics if activation ownership or processor publication fails.
    pub fn boundary(
        &mut self,
        resources: &MacDevices,
        exchange: &ProcessorExchange,
        processor: &mut Box<BlockProcessor>,
    ) -> bool {
        let mut activation = resources.activation().unwrap();
        activation.activate_recovered(&mut self.0);
        let installed = exchange
            .activate_pending_configured(processor, |_| {
                activation.activate(&mut self.0);
                Ok::<(), std::convert::Infallible>(())
            })
            .unwrap();
        drop(activation);
        resources.reclaim();
        exchange.reclaim_retired();
        installed
    }
    pub fn publish_recovery_now(&mut self, resources: &MacDevices) {
        publish_recovery_now(resources, &mut self.0);
    }
    pub fn wait_failed(&self) {
        wait_failed(&self.0);
    }
    #[must_use]
    pub fn bindings(&self) -> [String; 2] {
        bindings(&self.0)
    }
    #[must_use]
    pub fn retain_lifetime_observations(&self) -> OwnerLifetimes {
        OwnerLifetimes(self.0.0.values().map(Arc::downgrade).collect())
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.0.is_empty() && self.0.1.is_empty()
    }
    #[must_use]
    pub fn offline_is_empty(&self) -> bool {
        self.0.1.is_empty()
    }
    #[must_use]
    pub fn owner_count(&self) -> usize {
        self.0.0.len()
    }
    #[must_use]
    pub fn all_ready(&self) -> bool {
        self.0.0.values().all(|device| device.ready())
    }
    #[must_use]
    pub fn none_failed(&self) -> bool {
        self.0
            .0
            .values()
            .all(|device| !device.failed.load(Ordering::Acquire))
    }
    #[must_use]
    pub fn epoch(&self, id: &str) -> u64 {
        self.0.0[id].epoch.load(Ordering::Acquire)
    }
    #[must_use]
    pub fn owner_identity(&self, id: &str) -> usize {
        Arc::as_ptr(&self.0.0[id]) as usize
    }
    /// # Panics
    /// Panics if native owners, offline selections, or silent transfers differ.
    pub fn assert_published_offline(&self, graph: &GraphDocument, bound: &BTreeSet<String>) {
        let active = &self.0;
        assert!(active.0.is_empty());
        assert_eq!(active.1.keys().cloned().collect::<BTreeSet<_>>(), *bound);
        assert_eq!(
            by_direction(active.1.values()),
            by_direction(specs(graph).values())
        );
        let expected = TransferOutcome {
            activity: InputActivity::Inactive,
            ..TransferOutcome::complete(32)
        };
        let [input, output] = bindings(active);
        let mut pcm = [9.0; 64];
        assert_eq!(active.read(&input, &mut pcm, 0), Some(expected));
        assert_eq!(pcm.map(f32::to_bits), [0.0_f32; 64].map(f32::to_bits));
        assert_eq!(active.write(&output, &[0.5; 64], 0), Some(expected));
    }
    /// # Panics
    /// Panics if processing, callback pacing, PCM levels, or packet counts differ.
    pub fn assert_renders_installed_gain(
        &self,
        host: &SimulatedHost,
        processor: &mut BlockProcessor,
        gain_db: f32,
    ) {
        assert_presents(
            host,
            &self.0,
            48_000,
            &mut |input, output| {
                processor
                    .process_block(256, &[input], &mut [output])
                    .unwrap();
            },
            CAPTURED * 10_f32.powf(gain_db / 20.0),
        );
    }
}
