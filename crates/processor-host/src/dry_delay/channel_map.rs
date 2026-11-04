//! Which input channel feeds each output channel when a processor's dry signal stands in for it.

use arrayvec::ArrayVec;
use mixrelay_audio_graph::{
    MAX_PROVIDER_BUSES_PER_DIRECTION, ProviderBus, ProviderBusKind, ProviderPorts,
};

use super::DryDelayError;

/// A bus's position in the flattened channel order.
#[derive(Clone, Copy)]
struct ChannelSpan {
    start: usize,
    width: usize,
}

/// Maps each flattened output channel to its corresponding flattened main-input channel.
///
/// The n-th main output bus pairs with the n-th main input bus, channel by channel.
/// Sidechain/auxiliary outputs and unmatched main outputs map to `None` (silence).
///
/// # Errors
/// Returns a provider-port validation error for malformed or oversized topology.
pub(crate) fn main_bus_map(ports: &ProviderPorts) -> Result<Vec<Option<usize>>, DryDelayError> {
    ports.validate()?;
    let output_channels = usize::from(ports.total_output_channels()?);
    let mut mapping = Vec::new();
    mapping.try_reserve_exact(output_channels)?;
    mapping.resize(output_channels, None);

    let main_inputs = main_spans(&ports.inputs);
    let main_outputs = main_spans(&ports.outputs);
    for (input, output) in main_inputs.iter().zip(&main_outputs) {
        for channel in 0..input.width.min(output.width) {
            mapping[output.start + channel] = Some(input.start + channel);
        }
    }
    Ok(mapping)
}

/// Spans of the main buses in declaration order. Validated ports bound the bus count.
fn main_spans(buses: &[ProviderBus]) -> ArrayVec<ChannelSpan, MAX_PROVIDER_BUSES_PER_DIRECTION> {
    let mut spans = ArrayVec::new();
    let mut start = 0_usize;
    for bus in buses {
        let width = usize::from(bus.channels);
        if bus.kind == ProviderBusKind::Main {
            spans.push(ChannelSpan { start, width });
        }
        start += width;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::main_bus_map;
    use crate::dry_delay::tests::{bus, ports};
    use mixrelay_audio_graph::ProviderBusKind;

    #[test]
    fn provider_dry_map_pairs_main_bus_ordinals_and_excludes_auxiliary_buses() {
        let ports = ports(
            vec![
                bus("side-in", ProviderBusKind::Sidechain, 1),
                bus("main-in-0", ProviderBusKind::Main, 2),
                bus("aux-in", ProviderBusKind::Auxiliary, 2),
                bus("main-in-1", ProviderBusKind::Main, 1),
            ],
            vec![
                bus("aux-out", ProviderBusKind::Auxiliary, 2),
                bus("main-out-0", ProviderBusKind::Main, 2),
                bus("side-out", ProviderBusKind::Sidechain, 1),
                bus("main-out-1", ProviderBusKind::Main, 1),
                bus("unmatched-main", ProviderBusKind::Main, 1),
            ],
        );

        assert_eq!(
            main_bus_map(&ports).unwrap(),
            vec![None, None, Some(1), Some(2), None, Some(5), None]
        );
    }

    #[test]
    fn invalid_provider_topology_is_rejected_before_map_construction() {
        let invalid = ports(
            vec![
                bus("duplicate", ProviderBusKind::Main, 1),
                bus("duplicate", ProviderBusKind::Sidechain, 1),
            ],
            vec![bus("main-out", ProviderBusKind::Main, 1)],
        );
        assert!(main_bus_map(&invalid).is_err());
    }
}
