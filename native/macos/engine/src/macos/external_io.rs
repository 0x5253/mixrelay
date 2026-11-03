//! Graph input alignment and registry/device transfers at the native boundary.
#[cfg(test)]
mod tests;
use super::endpoint_service;
use crate::macos_pacing::Publication;
use mixrelay_audio_engine::{ExternalBinding, ExternalIo};
use mixrelay_audio_io::capture_alignment::CaptureFrontier;
use mixrelay_audio_io::process::{ProcessCycle, TransferOutcome};
use mixrelay_macos_transport::{
    application_clock::ConsumerRoute,
    source_registry::{GraphCredit, ReadMiss},
};

/// Time-addressed source reads without PCM, by reason, and the shared-cursor
/// reacquisitions they caused. Plain counters owned by the graph worker thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TimeReadMisses {
    pub(super) unavailable: u64,
    pub(super) not_ready: u64,
    pub(super) ahead: u64,
    pub(super) behind: u64,
    pub(super) mismatch: u64,
    pub(super) incomplete: u64,
    pub(super) reacquisitions: u64,
}
impl TimeReadMisses {
    fn record(&mut self, miss: ReadMiss) {
        let count = match miss {
            ReadMiss::Unavailable => &mut self.unavailable,
            ReadMiss::NotReady => &mut self.not_ready,
            ReadMiss::Ahead => &mut self.ahead,
            ReadMiss::Behind => &mut self.behind,
            ReadMiss::Mismatch => &mut self.mismatch,
            ReadMiss::Incomplete => &mut self.incomplete,
        };
        *count = count.saturating_add(1);
    }
}
impl std::ops::AddAssign for TimeReadMisses {
    fn add_assign(&mut self, other: Self) {
        self.unavailable = self.unavailable.saturating_add(other.unavailable);
        self.not_ready = self.not_ready.saturating_add(other.not_ready);
        self.ahead = self.ahead.saturating_add(other.ahead);
        self.behind = self.behind.saturating_add(other.behind);
        self.mismatch = self.mismatch.saturating_add(other.mismatch);
        self.incomplete = self.incomplete.saturating_add(other.incomplete);
        self.reacquisitions = self.reacquisitions.saturating_add(other.reacquisitions);
    }
}

pub(super) struct MacEngineExternalIo<'a> {
    pub(super) sources: Option<&'a mixrelay_macos_transport::source_registry::SourceRegistry>,
    pub(super) alignment: mixrelay_audio_io::capture_alignment::CaptureAlignment,
    capture_time: Option<u64>,
    pub(super) align_inputs: bool,
    /// Acquisition covers all pending clock credit; each transfer still uses cycle.frames.
    pub(super) capture_frames: u32,
    /// The pacing source publication that granted `capture_frames`.
    pub(super) publication: Option<Publication>,
    /// The shared source the pacer consumes from its first credit.
    pub(super) head: Option<u64>,
    /// This cycle's time-addressed read misses, collected by the worker after the cycle.
    pub(super) time_reads: TimeReadMisses,
    devices: &'a crate::macos_devices::DeviceSet,
    /// The admitted cycle's one host label for all of its device transfers.
    host: u64,
}

impl<'a> MacEngineExternalIo<'a> {
    pub(super) fn new(devices: &'a crate::macos_devices::DeviceSet) -> Self {
        Self {
            sources: endpoint_service::source_registry(),
            devices,
            host: 0,
            alignment: mixrelay_audio_io::capture_alignment::CaptureAlignment::default(),
            capture_time: None,
            align_inputs: false,
            capture_frames: 0,
            publication: None,
            head: None,
            time_reads: TimeReadMisses::default(),
        }
    }

    /// Counts a time-addressed miss. The pacing source's credit covers all PCM
    /// the graph consumes, so its cursor beyond the newest publication can never
    /// recover by waiting: the shared cursor reacquires at the next frontier.
    /// Every other miss stays an input shortage at the unchanged cursor.
    fn missed(&mut self, binding: &ExternalBinding, token: u64, miss: ReadMiss) {
        // A device binding without its native adapter has no registry source to miss.
        if shared_source(binding) {
            self.time_reads.record(miss);
        }
        if miss == ReadMiss::Ahead
            && self
                .publication
                .is_some_and(|publication| publication.token == token)
        {
            self.alignment.reacquire();
        }
    }

    fn source_active(&self, binding: &ExternalBinding) -> bool {
        let id = binding.node_id().as_str();
        self.devices.binding_present(id, true)
            || (shared_source(binding)
                && binding_sources(binding, self.sources)
                    .and_then(|sources| sources.clock(binding_token(binding)))
                    .is_some_and(|clock| clock.running != 0))
    }
}

impl ExternalIo for MacEngineExternalIo<'_> {
    fn source_epoch(&self, binding: &ExternalBinding) -> Option<u64> {
        if self
            .devices
            .binding_present(binding.node_id().as_str(), true)
        {
            None
        } else {
            application_epoch(binding, self.sources)
        }
    }

    fn begin_cycle(
        &mut self,
        cycle: &ProcessCycle,
        controls: mixrelay_audio_engine::ExternalControlState<'_>,
    ) -> mixrelay_audio_engine::CycleAdmission {
        self.host = self.devices.graph_host();
        for (binding, enabled) in controls.sources() {
            self.devices
                .set_binding_active(binding.node_id().as_str(), true, enabled);
        }
        for (binding, enabled) in controls.sinks() {
            self.devices
                .set_binding_active(binding.node_id().as_str(), false, enabled);
        }
        publish_routes(&controls, self.head);
        let mut active = 0;
        let mut latest = None::<CaptureFrontier>;
        let mut source_period = None;
        let count = controls
            .sources()
            .filter(|(binding, enabled)| {
                *enabled && binding.is_routed_source() && self.source_active(binding)
            })
            .count();
        let capture_frames = self.capture_frames.max(cycle.frames);
        self.devices
            .reserve_capture_alignment(cycle.sample_rate, cycle.frames, count > 0);
        for (binding, _) in controls
            .sources()
            .filter(|(binding, enabled)| *enabled && binding.is_routed_source())
        {
            let id = binding.node_id().as_str();
            let token = binding_token(binding);
            let sources = binding_sources(binding, self.sources);
            // The pacing source's frontier follows the publication its credit
            // covers, not PCM committed after the pacer's snapshot.
            let credit = GraphCredit {
                frames: cycle.frames,
                available_frames: capture_frames,
                published: self
                    .publication
                    .filter(|publication| publication.token == token)
                    .map(|publication| publication.credit),
            };
            let time = self.devices.capture_time(id, capture_frames).or_else(|| {
                sources.and_then(|sources| {
                    sources.source_time_for_credit(token, cycle.sample_rate, credit)
                })
            });
            if self.source_active(binding) {
                active += 1;
            }
            if let Some(time) = time
                && latest.is_none_or(|old| time < old.nanos())
            {
                let epoch = self.source_epoch(binding).unwrap_or(0);
                let identity =
                    hash_source(token.to_ne_bytes().into_iter().chain(epoch.to_ne_bytes()));
                latest = Some(CaptureFrontier::new(identity, time));
            }
            source_period =
                source_period.or_else(|| sources.and_then(|sources| sources.source_period(token)));
        }
        self.align_inputs = active > 0;
        self.capture_time = if self.align_inputs {
            // A source without valid coordinates remains silent independently;
            // it must not hold every other route in startup priming forever.
            let period = self
                .devices
                .frame_period(cycle.sample_rate)
                .or(source_period)
                .unwrap_or(1e9 / f64::from(cycle.sample_rate));
            let time = self.alignment.begin(*cycle, latest, period);
            self.time_reads.reacquisitions += u64::from(self.alignment.reacquired());
            time
        } else {
            None
        };
        mixrelay_audio_engine::CycleAdmission::Process
    }

    fn read_source(
        &mut self,
        binding: &ExternalBinding,
        destination: &mut [f32],
        cycle: &ProcessCycle,
    ) -> TransferOutcome {
        if !self
            .devices
            .binding_present(binding.node_id().as_str(), true)
            && virtual_source_idle(
                binding,
                binding_sources(binding, self.sources),
                (cycle.sample_rate, cycle.frames),
            )
        {
            destination.fill(0.0);
            return TransferOutcome {
                activity: mixrelay_audio_io::process::InputActivity::Inactive,
                ..TransferOutcome::complete(0)
            };
        }
        if self.align_inputs && binding.is_routed_source() {
            if let Some(time) = self.capture_time {
                // A reacquired shared cursor resets the history each aligned input reaches.
                let discontinuity = self.alignment.reacquired();
                if let Some(mut outcome) = self.devices.read_aligned(
                    binding.node_id().as_str(),
                    destination,
                    (self.host, time, cycle.sample_rate),
                ) {
                    outcome.discontinuity |= discontinuity;
                    return outcome;
                }
                let token = binding_token(binding);
                let sources = binding_sources(binding, self.sources);
                let available = if let Some(sources) = sources {
                    sources
                        .read_audio_timed(
                            token,
                            destination,
                            cycle.frames,
                            mixrelay_macos_transport::source_registry::content_latency::ReadTiming {
                                rate: cycle.sample_rate,
                                first_nanos: Some(time),
                                host_ticks: Some(self.host),
                            },
                        )
                        .unwrap_or_else(|miss| {
                            self.missed(binding, token, miss);
                            0
                        })
                } else {
                    destination.fill(0.0);
                    0
                };
                return TransferOutcome {
                    discontinuity,
                    source_sample_faults: sources
                        .map_or(0, |sources| sources.take_source_faults(token)),
                    ..TransferOutcome::complete(available)
                };
            }
            destination.fill(0.0);
            return TransferOutcome {
                activity: mixrelay_audio_io::process::InputActivity::Priming,
                ..TransferOutcome::complete(0)
            };
        }
        if let Some(outcome) = self
            .devices
            .read(binding.node_id().as_str(), destination, self.host)
        {
            return outcome;
        }
        read_application(binding, destination, cycle, self.sources, Some(self.host))
    }

    fn write_sink(
        &mut self,
        binding: &ExternalBinding,
        source: &[f32],
        _cycle: &ProcessCycle,
    ) -> TransferOutcome {
        self.devices
            .write(binding.node_id().as_str(), source, self.host)
            .unwrap_or_else(|| TransferOutcome::complete(0))
    }
}

/// Publishes how this plan uses each virtual source to its producer.
fn publish_routes(controls: &mixrelay_audio_engine::ExternalControlState<'_>, head: Option<u64>) {
    if let Some(sources) = endpoint_service::virtual_source_registry() {
        for (token, route) in plan_routes(controls.sources(), head) {
            sources.set_consumer_route(token, route);
        }
    }
}

/// How a plan uses each virtual source. A source is routed while its node is on and
/// the plan carries it to an output. `head` is the source the pacer consumes from its
/// first credit; only that source receives PCM retained before admission.
pub(super) fn plan_routes<'a>(
    sources: impl Iterator<Item = (&'a ExternalBinding, bool)>,
    head: Option<u64>,
) -> impl Iterator<Item = (u64, ConsumerRoute)> {
    sources.filter_map(move |(binding, enabled)| {
        binding.virtual_endpoint_id()?;
        let token = binding_token(binding);
        Some((
            token,
            if enabled && binding.is_routed_source() {
                ConsumerRoute::Routed {
                    head: head == Some(token),
                }
            } else {
                ConsumerRoute::Unrouted
            },
        ))
    })
}

/// Application captures and virtual endpoints are served by a source registry.
fn shared_source(binding: &ExternalBinding) -> bool {
    binding.kind() == mixrelay_audio_graph::NodeKind::ApplicationCapture
        || binding.virtual_endpoint_id().is_some()
}

pub(super) fn application_epoch(
    binding: &ExternalBinding,
    sources: Option<&mixrelay_macos_transport::source_registry::SourceRegistry>,
) -> Option<u64> {
    shared_source(binding).then(|| {
        binding_sources(binding, sources)
            .and_then(|sources| sources.clock(binding_token(binding)))
            .filter(|clock| clock.running != 0)
            .map_or(0, |clock| clock.epoch)
    })
}

pub(super) fn read_application(
    binding: &ExternalBinding,
    destination: &mut [f32],
    cycle: &ProcessCycle,
    sources: Option<&mixrelay_macos_transport::source_registry::SourceRegistry>,
    host: Option<u64>,
) -> TransferOutcome {
    read_bound_source(
        binding,
        destination,
        cycle,
        binding_sources(binding, sources),
        host,
    )
}

fn read_bound_source(
    binding: &ExternalBinding,
    destination: &mut [f32],
    cycle: &ProcessCycle,
    sources: Option<&mixrelay_macos_transport::source_registry::SourceRegistry>,
    host: Option<u64>,
) -> TransferOutcome {
    if virtual_source_idle(binding, sources, (cycle.sample_rate, cycle.frames)) {
        destination.fill(0.0);
        return TransferOutcome {
            activity: mixrelay_audio_io::process::InputActivity::Inactive,
            ..TransferOutcome::complete(0)
        };
    }
    let token = binding_token(binding);
    let available = if let Some(sources) = sources {
        sources.read_audio_time(
            token,
            destination,
            cycle.frames,
            mixrelay_macos_transport::source_registry::content_latency::ReadTiming {
                rate: cycle.sample_rate,
                first_nanos: None,
                host_ticks: host,
            },
        )
    } else {
        destination.fill(0.0);
        0
    };
    TransferOutcome {
        source_sample_faults: sources.map_or(0, |sources| sources.take_source_faults(token)),
        ..TransferOutcome::complete(available)
    }
}

fn virtual_source_idle(
    binding: &ExternalBinding,
    sources: Option<&mixrelay_macos_transport::source_registry::SourceRegistry>,
    (rate, frames): (u32, u32),
) -> bool {
    if binding.virtual_endpoint_id().is_none() {
        return false;
    }
    let Some(sources) = sources else {
        return true;
    };
    let token = binding_token(binding);
    // Unrouted sources also negotiate their graph format before the first PCM,
    // so their meters cannot deadlock behind an unacknowledged running clock.
    let _ = sources.source_time(token, rate, frames);
    sources.clock(token).is_none_or(|clock| clock.running == 0)
}

pub(super) fn source_token(source_id: &str) -> u64 {
    hash_source(source_id.bytes())
}

fn hash_source(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut hash = 14_695_981_039_346_656_037_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    if hash == 0 { 1 } else { hash }
}

pub(super) fn binding_token(binding: &ExternalBinding) -> u64 {
    binding.virtual_endpoint_id().map_or_else(
        || source_token(binding.node_id().as_str()),
        |id| hash_source("virtual:".bytes().chain(id.as_str().bytes())),
    )
}

pub(super) fn binding_sources<'a>(
    binding: &ExternalBinding,
    applications: Option<&'a mixrelay_macos_transport::source_registry::SourceRegistry>,
) -> Option<&'a mixrelay_macos_transport::source_registry::SourceRegistry> {
    if binding.virtual_endpoint_id().is_some() {
        endpoint_service::virtual_source_registry()
    } else {
        applications
    }
}
