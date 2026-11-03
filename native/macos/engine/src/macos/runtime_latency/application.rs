//! Application PCM ages enter the same destination windows as native device taps.
use super::Observation;
use mixrelay_audio_engine::{BlockProcessor, RenderPlan};
use mixrelay_audio_graph::NodeKind;
use mixrelay_audio_io::residence::ResidenceSnapshot;
use mixrelay_macos_transport::{device_io::LatencySnapshot, source_registry::SourceRegistry};
use std::collections::BTreeMap;

pub(crate) fn record_sources(
    processor: &BlockProcessor,
    sources: Option<&SourceRegistry>,
    completed: bool,
    epoch: u64,
) {
    let _ = processor.acknowledge_content_latency();
    for source in processor.plan().source_bindings().iter().filter(|source| {
        source.kind() == NodeKind::ApplicationCapture || source.virtual_endpoint_id().is_some()
    }) {
        let id = source.node_id().as_str();
        let Some(sources) = super::super::binding_sources(source, sources) else {
            continue;
        };
        sources.record_content_latency(
            super::super::binding_token(source),
            processor.plan().revision(),
            epoch,
            completed && processor.binding_content_latency_ready(id, true),
        );
    }
}

pub(super) fn append_observations(
    plan: &RenderPlan,
    sources: Option<&SourceRegistry>,
    pending: bool,
    observations: &mut BTreeMap<String, Observation>,
) {
    for source in plan.source_bindings().iter().filter(|source| {
        source.kind() == NodeKind::ApplicationCapture || source.virtual_endpoint_id().is_some()
    }) {
        let id = source.node_id().as_str();
        let Some(sources) = super::super::binding_sources(source, sources) else {
            continue;
        };
        let Some(snapshot) = sources.content_latency(super::super::binding_token(source)) else {
            continue;
        };
        let context = snapshot.context;
        let graph_rate = u32::try_from(context.format & u64::from(u32::MAX)).unwrap_or(0);
        observations.insert(
            id.into(),
            Observation {
                // Registry leases are unique, bounded to 48 bits. Reserve the high bit
                // to keep source identities separate from native device owner counters.
                identity: (1 << 63)
                    | (u64::from(source.virtual_endpoint_id().is_some()) << 62)
                    | context.lease,
                epoch: context.epoch,
                graph_epoch: context.graph_epoch,
                generation: context.format,
                clock_breaks: 0,
                native_rate: context.native_rate,
                graph_rate,
                direct: false,
                ready: !pending
                    && context.ready
                    && context.revision == plan.revision()
                    && context.running
                    && context.native_rate != 0
                    && graph_rate != 0
                    && context.lease != 0
                    && context.epoch != 0
                    && context.format != 0
                    && context.format == context.acknowledged,
                sampled: snapshot.age.is_some(),
                faults: snapshot.faults,
                native: LatencySnapshot {
                    content_age: snapshot.age.unwrap_or_default(),
                    ..Default::default()
                },
                src_input: ResidenceSnapshot::default(),
                src_output: ResidenceSnapshot::default(),
                src_delay: 0,
            },
        );
    }
}
