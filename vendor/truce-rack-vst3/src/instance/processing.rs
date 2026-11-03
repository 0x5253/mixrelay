//! Run synchronous VST3 audio calls with retained parameter and event storage.

use super::Vst3Plugin;
use crate::{
    FORMAT, bus_layout, event_exchange, parameter_delivery, process_context::build_vst3_context,
};
use std::ptr;
use truce_rack_core::{
    buffer::AudioBuffer,
    error::{Error, ProcessFault, Result},
    events::EventList,
    plugin::{Plugin, ProcessContext, ProcessStatus},
    wrapper::run_audio_block_with,
};
use vst3::Steinberg::{
    Vst::{IParameterChanges, ProcessData, ProcessModes_, ProcessSetup, SymbolicSampleSizes_},
    kResultOk,
};

pub(super) struct ProcessingState {
    pub(super) native_layouts: Vec<bus_layout::NativeLayout>,
    pub(super) active_layout: Option<bus_layout::NativeLayout>,
    pub(super) latency_frames: Option<u32>,
    pub(super) activation_setup: Option<ProcessSetup>,
    pub(super) input_parameters: parameter_delivery::InputParameters,
    pub(super) processing: bool,
    pub(super) event_exchange: event_exchange::EventExchange,
}
impl Plugin<f32> for Vst3Plugin {
    fn process(
        &mut self,
        buffer: &mut AudioBuffer<'_, f32>,
        events: &EventList,
        context: &mut ProcessContext<'_>,
    ) -> Result<ProcessStatus> {
        let layout = self
            .processing
            .active_layout
            .as_ref()
            .ok_or(Error::NotActivated)?;
        let frames = buffer.num_frames();
        let Some(setup) = self.processing.activation_setup.as_ref() else {
            return Err(Error::Process {
                format: FORMAT,
                fault: ProcessFault::InvalidBlockSize,
            });
        };
        if !self.processing.processing || !accepts_block(setup, context, frames) {
            return Err(Error::Process {
                format: FORMAT,
                fault: ProcessFault::InvalidBlockSize,
            });
        }

        // Build the transport context up front so its backing
        // struct outlives the process call.
        let mut process_context = context
            .transport
            .map(|t| build_vst3_context(&t, context.sample_rate));

        let exchange = &mut self.processing.event_exchange;
        let parameters = &mut self.processing.input_parameters;
        let processor = &self.native.processor;
        let status = bus_layout::with_buffers(buffer, layout, |inputs, outputs| {
            exchange.process(
                events,
                frames,
                context.output_events,
                |input_events_ptr, output_events_ptr| {
                    let mut data = ProcessData {
                        processMode: i32::from_ne_bytes(ProcessModes_::kRealtime.to_ne_bytes()),
                        symbolicSampleSize: i32::from_ne_bytes(
                            SymbolicSampleSizes_::kSample32.to_ne_bytes(),
                        ),
                        numSamples: i32::try_from(frames).unwrap_or(i32::MAX),
                        numInputs: i32::try_from(inputs.len()).unwrap_or_default(),
                        numOutputs: i32::try_from(outputs.len()).unwrap_or_default(),
                        inputs: if inputs.is_empty() {
                            ptr::null_mut()
                        } else {
                            inputs.as_mut_ptr()
                        },
                        outputs: if outputs.is_empty() {
                            ptr::null_mut()
                        } else {
                            outputs.as_mut_ptr()
                        },
                        inputParameterChanges: ptr::null_mut::<IParameterChanges>(),
                        outputParameterChanges: ptr::null_mut::<IParameterChanges>(),
                        inputEvents: input_events_ptr,
                        outputEvents: output_events_ptr,
                        processContext: process_context
                            .as_mut()
                            .map_or(ptr::null_mut(), std::ptr::from_mut),
                    };

                    run_audio_block_with::<Vst3Plugin, i32>(FORMAT, -1, || {
                        parameters.process(processor, &mut data)
                    })
                },
            )
        })?
        .map_err(|fault| Error::Process {
            format: FORMAT,
            fault,
        })?;
        if status == kResultOk {
            Ok(ProcessStatus::Continue)
        } else {
            Err(Error::Process {
                format: FORMAT,
                fault: ProcessFault::NativeStatus(i64::from(status)),
            })
        }
    }
}

fn accepts_block(setup: &ProcessSetup, context: &ProcessContext<'_>, frames: usize) -> bool {
    setup.sampleRate.is_finite()
        && setup.sampleRate > 0.0
        && context.sample_rate.to_bits() == setup.sampleRate.to_bits()
        && usize::try_from(setup.maxSamplesPerBlock).is_ok_and(|maximum| frames <= maximum)
        && frames <= context.max_block_size
}

#[cfg(test)]
mod tests;
