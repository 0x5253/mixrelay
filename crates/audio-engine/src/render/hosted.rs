//! Prepared hosted processor ownership, reuse and multibus PCM transfer.
use mixrelay_audio_graph::{NodeId, ProviderNodeConfig, RenderFormat};
use std::{fmt, sync::Arc};

use super::{
    DryBusMap, HostedIo, HostedLatencySource, HostedProcessors, PortSlot, RenderPlan,
    RenderPlanError, buffers, port_pair, sample_count,
};

mod identity;
pub use identity::HostedProcessorIdentity;

/// A fully prepared third-party processor owned by the audio worker.
///
/// Implementations must reuse storage and must not wait on the UI or control plane from
/// `process`. A processor without a usable output requests the graph's channel-preserving
/// fallback. Buffered processors must supply their own latency-aligned fallback instead.
pub trait HostedAudioProcessor: Send {
    /// Renders `frames` of interleaved input into the corresponding interleaved output.
    /// Returns `true` when output is complete, including intentional silence or an aligned
    /// dry fallback; `false` requests the graph's immediate dry fallback. A complete output
    /// does not mean the native plug-in is healthy; health is observed separately.
    fn process(&mut self, input: &[f32], output: &mut [f32], frames: usize) -> bool;

    /// Discards buffered audio history while this node is OFF.
    ///
    /// Called from the audio thread and may be called on every disabled callback. Implementations
    /// must not allocate, lock, block, or call native plug-in code. This suspends transport history
    /// only; it does not transfer ownership or change graph state. The default is appropriate for
    /// processors without buffered transport.
    fn suspend_processing(&mut self) {}

    /// Borrows the exact instance key shared with its inactive ownership requests.
    /// Called during activation; implementations must not allocate, lock or update ownership.
    fn reuse_identity(&self) -> Option<&HostedProcessorIdentity> {
        None
    }

    /// An inactive ownership-transfer request, not a renderable processor.
    fn requests_reuse(&self) -> bool {
        false
    }

    /// Publishes ownership only; must not allocate, lock, or call native code.
    fn set_active(&mut self, _active: bool) {}

    /// Returns a retained cached-latency reader for this exact prepared worker.
    /// Called only during control-thread preparation, never while rendering.
    /// Missing readers mean unknown latency, not a reported zero.
    fn latency_source(&self) -> Option<Arc<dyn HostedLatencySource>> {
        None
    }
}

/// Immutable preparation data passed across the engine/provider boundary.
pub struct HostedProcessorPrepareRequest<'a> {
    pub node_id: &'a NodeId,
    pub config: &'a ProviderNodeConfig,
    pub format: RenderFormat,
    pub input_channels: u16,
    pub output_channels: u16,
}

/// Creates hosted processors while a graph is prepared on the control thread.
pub trait HostedProcessorFactory: Send + Sync {
    /// Loads and configures one provider node for the current render format.
    ///
    /// # Errors
    ///
    /// Returns a stable diagnostic when the provider is missing or cannot be prepared.
    fn prepare(
        &self,
        request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Box<dyn HostedAudioProcessor>, String>;

    /// Retains an already loaded inactive node during an unrelated topology edit.
    /// Never instantiates a new provider here: `None` keeps an initially OFF node unprepared.
    ///
    /// # Errors
    /// Returns a diagnostic if an existing reusable owner cannot be retained safely.
    fn prepare_inactive(
        &self,
        _request: HostedProcessorPrepareRequest<'_>,
    ) -> Result<Option<Box<dyn HostedAudioProcessor>>, String> {
        Ok(None)
    }
}

pub(super) struct HostedRack {
    processors: HostedProcessors,
    identities: Box<[(HostedProcessorIdentity, usize)]>,
    scratch: Box<[f32]>,
    input_scratch_samples: usize,
}

impl fmt::Debug for HostedRack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedRack")
            .field("processors", &self.processors.len())
            .field("identities", &self.identities)
            .field("scratch_samples", &self.scratch.len())
            .field("input_scratch_samples", &self.input_scratch_samples)
            .finish()
    }
}

impl HostedRack {
    pub(super) fn new(
        plan: &RenderPlan,
        processors: HostedProcessors,
    ) -> Result<Self, RenderPlanError> {
        let mut identities = buffers::reserve(processors.len())?;
        identities.extend(
            processors
                .iter()
                .enumerate()
                .filter_map(|(index, processor)| {
                    processor.reuse_identity().cloned().map(|id| (id, index))
                }),
        );
        identities.sort_unstable();
        Ok(Self {
            processors,
            identities: identities.into_boxed_slice(),
            scratch: buffers::filled(plan.hosted_scratch_samples, 0.0)?,
            input_scratch_samples: plan.hosted_input_scratch_samples,
        })
    }

    pub(super) fn can_adopt(&self, previous: &Self) -> bool {
        self.identities.iter().all(|(identity, index)| {
            !self.processors[*index].requests_reuse()
                || previous
                    .identities
                    .binary_search_by(|(id, _)| id.cmp(identity))
                    .is_ok_and(|found| {
                        !previous.processors[previous.identities[found].1].requests_reuse()
                    })
        })
    }

    pub(super) fn adopt(&mut self, previous: &mut Self) {
        for (identity, index) in &self.identities {
            if self.processors[*index].requests_reuse() {
                let Ok(found) = previous
                    .identities
                    .binary_search_by(|(id, _)| id.cmp(identity))
                else {
                    continue;
                };
                let previous_index = previous.identities[found].1;
                std::mem::swap(
                    &mut self.processors[*index],
                    &mut previous.processors[previous_index],
                );
            }
        }
    }

    pub(super) fn set_active(&mut self, active: bool) {
        for processor in &mut self.processors {
            processor.set_active(active);
        }
    }

    pub(super) fn suspend(&mut self, index: usize) {
        self.processors[index].suspend_processing();
    }

    #[cfg(test)]
    pub(super) fn scratch_samples(&self) -> usize {
        self.scratch.len()
    }

    pub(super) fn process(
        &mut self,
        arena: &mut [f32],
        io: &HostedIo,
        index: usize,
        frames: usize,
    ) {
        match io {
            HostedIo::Single { input, output } => apply_hosted_processor(
                arena,
                *input,
                *output,
                frames,
                self.processors[index].as_mut(),
            ),
            HostedIo::Multiple {
                inputs,
                outputs,
                input_channels,
                output_channels,
                dry_maps,
            } => {
                apply_hosted_processor_multiple(
                    arena,
                    MultipleHostedIo {
                        inputs,
                        outputs,
                        input_channels: *input_channels,
                        output_channels: *output_channels,
                        frames,
                        dry_maps,
                    },
                    HostedScratch {
                        samples: &mut self.scratch,
                        input_samples: self.input_scratch_samples,
                    },
                    self.processors[index].as_mut(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;

fn apply_hosted_processor(
    arena: &mut [f32],
    input: PortSlot,
    output: PortSlot,
    frames: usize,
    processor: &mut dyn HostedAudioProcessor,
) {
    let (source, destination) = port_pair(arena, input, output, frames);
    let processed = processor.process(source, destination, frames);
    if processed {
        return;
    }
    let shared_channels = usize::from(input.channels.min(output.channels));
    for frame in 0..frames {
        for channel in 0..usize::from(output.channels) {
            arena[output.offset + frame * usize::from(output.channels) + channel] =
                if channel < shared_channels {
                    arena[input.offset + frame * usize::from(input.channels) + channel]
                } else {
                    0.0
                };
        }
    }
}

#[derive(Clone, Copy)]
struct MultipleHostedIo<'a> {
    inputs: &'a [PortSlot],
    outputs: &'a [PortSlot],
    input_channels: u16,
    output_channels: u16,
    frames: usize,
    dry_maps: &'a [DryBusMap],
}

struct HostedScratch<'a> {
    samples: &'a mut [f32],
    input_samples: usize,
}

fn apply_hosted_processor_multiple(
    arena: &mut [f32],
    io: MultipleHostedIo<'_>,
    scratch: HostedScratch<'_>,
    processor: &mut dyn HostedAudioProcessor,
) {
    let input_samples = sample_count(io.frames, io.input_channels);
    let output_samples = sample_count(io.frames, io.output_channels);
    let HostedScratch {
        samples,
        input_samples: scratch_input_samples,
    } = scratch;
    let (input_scratch, output_scratch) = samples.split_at_mut(scratch_input_samples);
    let input = &mut input_scratch[..input_samples];
    let output = &mut output_scratch[..output_samples];
    gather_buses(arena, io.inputs, input, io.frames, io.input_channels);
    if !processor.process(input, output, io.frames) {
        preserve_main_buses(input, output, &io);
    }
    scatter_buses(arena, io.outputs, output, io.frames, io.output_channels);
}

fn gather_buses(
    arena: &[f32],
    buses: &[PortSlot],
    destination: &mut [f32],
    frames: usize,
    total_channels: u16,
) {
    let mut channels_before = 0;
    for frame in 0..frames {
        for bus in buses {
            let channels = usize::from(bus.channels);
            let source_start = bus.offset + frame * channels;
            let destination_start = frame * usize::from(total_channels) + channels_before;
            destination[destination_start..destination_start + channels]
                .copy_from_slice(&arena[source_start..source_start + channels]);
            channels_before += channels;
        }
        channels_before = 0;
    }
}

fn scatter_buses(
    arena: &mut [f32],
    buses: &[PortSlot],
    source: &[f32],
    frames: usize,
    total_channels: u16,
) {
    let mut channels_before = 0;
    for frame in 0..frames {
        for bus in buses {
            let channels = usize::from(bus.channels);
            let source_start = frame * usize::from(total_channels) + channels_before;
            let destination_start = bus.offset + frame * channels;
            arena[destination_start..destination_start + channels]
                .copy_from_slice(&source[source_start..source_start + channels]);
            channels_before += channels;
        }
        channels_before = 0;
    }
}

fn preserve_main_buses(input: &[f32], output: &mut [f32], io: &MultipleHostedIo<'_>) {
    output.fill(0.0);
    let input_channels = usize::from(io.input_channels);
    let output_channels = usize::from(io.output_channels);
    for mapping in io.dry_maps {
        for frame in 0..io.frames {
            let source = frame * input_channels + mapping.source_channel_offset;
            let destination = frame * output_channels + mapping.destination_channel_offset;
            let channels = usize::from(mapping.channels);
            output[destination..destination + channels]
                .copy_from_slice(&input[source..source + channels]);
        }
    }
}
