//! Retain bounded COM event storage across audio blocks.

use truce_rack_core::{
    error::{ProcessFault, Result},
    events::EventList,
};
use vst3::{ComPtr, ComWrapper, Steinberg::Vst::IEventList};

mod output;
mod storage;
mod translation;

/// One note on and off for every MIDI 1.0 channel/pitch fits in a single block.
/// This is a storage bound, independent of the audio frame quantum.
const MAX_EVENTS: usize = 16 * 128 * 2;

pub(super) struct EventExchange {
    input: ComWrapper<storage::EventStorage>,
    input_ptr: ComPtr<IEventList>,
    output: ComWrapper<storage::EventStorage>,
    output_ptr: ComPtr<IEventList>,
}

impl EventExchange {
    pub(super) fn new() -> Result<Self> {
        let input = ComWrapper::new(storage::EventStorage::new(false)?);
        let output = ComWrapper::new(storage::EventStorage::new(true)?);
        let input_ptr = input
            .to_com_ptr::<IEventList>()
            .ok_or_else(storage::interface_error)?;
        let output_ptr = output
            .to_com_ptr::<IEventList>()
            .ok_or_else(storage::interface_error)?;
        Ok(Self {
            input,
            input_ptr,
            output,
            output_ptr,
        })
    }

    pub(super) fn process<T>(
        &mut self,
        events: &EventList,
        frames: usize,
        output: &mut EventList,
        process: impl FnOnce(*mut IEventList, *mut IEventList) -> T,
    ) -> std::result::Result<T, ProcessFault> {
        output.clear();
        let input_faults = self.input.faults();
        let input_capacity = self.input.capacity_faults();
        let output_faults = self.output.faults();
        let output_capacity = self.output.capacity_faults();
        if !self.input.reset(frames) {
            return Err(ProcessFault::InvalidInputEvent);
        }
        if !self.output.reset(frames) {
            return Err(ProcessFault::InvalidOutputEvent);
        }
        for event in events {
            if let Some(event) = translation::translate(event, frames)
                .map_err(|()| ProcessFault::InvalidInputEvent)?
                && !self.input.push(event)
            {
                return Err(if self.input.capacity_faults() == input_capacity {
                    ProcessFault::InvalidInputEvent
                } else {
                    ProcessFault::InputEventCapacity
                });
            }
        }
        let result = process(self.input_ptr.as_ptr(), self.output_ptr.as_ptr());
        if self.input.faults() != input_faults {
            return Err(ProcessFault::InvalidInputEvent);
        }
        if self.output.faults() != output_faults {
            return Err(if self.output.capacity_faults() == output_capacity {
                ProcessFault::InvalidOutputEvent
            } else {
                ProcessFault::OutputEventCapacity
            });
        }
        if !self.output.for_each(|event| output::forward(event, output)) {
            return Err(if self.output.faults() == output_faults {
                ProcessFault::OutputEventCapacity
            } else {
                ProcessFault::InvalidOutputEvent
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
