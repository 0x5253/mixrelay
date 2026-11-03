use crate::instance_id::InstanceId;
use crate::{
    AudioLayout, EDITOR_COMMAND_NONE, EDITOR_FAILURE_NONE, EDITOR_STATE_UNAVAILABLE,
    PARAMETER_CAPACITY, SHARED_MAGIC, SHARED_VERSION, STATE_OK, WORKER_STARTING, editor_surface,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
/// Header shared by the engine and one isolated processor worker.
///
/// The PCM regions following the bounded state chunk are protected by the sequence atomics: the
/// producer never overwrites a region until the consumer publishes the matching consumed
/// sequence. State responses use cumulative chunk progress and a final generation acknowledgement.
#[repr(C, align(64))]
pub(crate) struct SharedHeader {
    pub(super) magic: u64,
    pub(super) version: u32,
    pub(super) input_channels: u32,
    pub(super) output_channels: u32,
    pub(super) block_frames: u32,
    pub(crate) intrinsic_latency_frames: AtomicU64,
    pub(crate) worker_status: AtomicU32,
    pub(crate) shutdown: AtomicU32,
    pub(super) input_sequence: AtomicU64,
    pub(super) input_consumed: AtomicU64,
    pub(super) input_first_frame: AtomicU64,
    pub(super) input_frames: AtomicU32,
    pub(super) output_sequence: AtomicU64,
    pub(super) output_consumed: AtomicU64,
    pub(super) output_first_frame: AtomicU64,
    pub(super) output_frames: AtomicU32,
    pub(crate) process_faults: crate::process_fault::ProcessFaults,
    pub(crate) native_operation: crate::native_operation::NativeOperation,
    pub(crate) dsp_operation: crate::native_operation::NativeOperation,
    pub(crate) parameter_count: AtomicU32,
    pub(crate) parameters_pending: AtomicBool,
    pub(crate) parameters: [ParameterSlot; PARAMETER_CAPACITY],
    pub(super) state_request: AtomicU64,
    pub(super) state_kind: AtomicU32,
    pub(super) state_response: AtomicU64,
    pub(super) state_written: AtomicU32,
    pub(super) state_consumed: AtomicU32,
    pub(super) state_cancel: AtomicU64,
    pub(super) state_length: AtomicU32,
    pub(super) state_result: AtomicU32,
    pub(crate) editor_command: AtomicU32,
    pub(crate) editor_sequence: AtomicU64,
    pub(crate) editor_consumed: AtomicU64,
    pub(crate) editor_state: AtomicU32,
    pub(crate) editor_failure: AtomicU32,
    pub(crate) editor_width: AtomicU32,
    pub(crate) editor_height: AtomicU32,
    pub(crate) editor_resizable: AtomicU32,
    pub(crate) instance_id: u64,
    /// Broker-owned lifecycle, separate from the parent's optimistic Opening status.
    pub(crate) editor_observed_open: AtomicBool,
    pub(crate) editor_close_generation: AtomicU64,
    pub(crate) editor_input: editor_surface::InputRing,
}

/// Slots are assigned once per worker lifetime by serialized control publishers. Stable IDs
/// let the consumer load each atomic value independently, without a torn ID/value pair.
#[repr(C)]
pub(crate) struct ParameterSlot {
    pub(crate) id: AtomicU32,
    pub(crate) value_bits: AtomicU64,
    pub(crate) pending: AtomicBool,
}

impl ParameterSlot {
    fn empty() -> Self {
        Self {
            id: AtomicU32::new(0),
            value_bits: AtomicU64::new(0),
            pending: AtomicBool::new(false),
        }
    }
}

impl SharedHeader {
    /// Builds the initial header for `layout`.
    ///
    /// # Errors
    /// Returns an error when a transport dimension does not fit the u32 header fields.
    pub(super) fn new(layout: AudioLayout, instance_id: InstanceId) -> Result<Self, String> {
        let dimension = |count: usize| {
            u32::try_from(count).map_err(|_| "processor transport dimensions exceed u32".to_owned())
        };
        Ok(Self {
            magic: SHARED_MAGIC,
            version: SHARED_VERSION,
            input_channels: dimension(layout.inputs)?,
            output_channels: dimension(layout.outputs)?,
            block_frames: dimension(layout.block_frames)?,
            intrinsic_latency_frames: AtomicU64::new(0),
            worker_status: AtomicU32::new(WORKER_STARTING),
            shutdown: AtomicU32::new(0),
            input_sequence: AtomicU64::new(0),
            input_consumed: AtomicU64::new(0),
            input_first_frame: AtomicU64::new(0),
            input_frames: AtomicU32::new(0),
            output_sequence: AtomicU64::new(0),
            output_consumed: AtomicU64::new(0),
            output_first_frame: AtomicU64::new(0),
            output_frames: AtomicU32::new(0),
            process_faults: crate::process_fault::ProcessFaults::new(),
            native_operation: crate::native_operation::NativeOperation::new(),
            dsp_operation: crate::native_operation::NativeOperation::new(),
            parameter_count: AtomicU32::new(0),
            parameters_pending: AtomicBool::new(false),
            parameters: std::array::from_fn(|_| ParameterSlot::empty()),
            state_request: AtomicU64::new(0),
            state_kind: AtomicU32::new(editor_surface::NATIVE_STATE),
            state_response: AtomicU64::new(0),
            state_written: AtomicU32::new(0),
            state_consumed: AtomicU32::new(0),
            state_cancel: AtomicU64::new(0),
            state_length: AtomicU32::new(0),
            state_result: AtomicU32::new(STATE_OK),
            editor_command: AtomicU32::new(EDITOR_COMMAND_NONE),
            editor_sequence: AtomicU64::new(0),
            editor_consumed: AtomicU64::new(0),
            editor_state: AtomicU32::new(EDITOR_STATE_UNAVAILABLE),
            editor_failure: AtomicU32::new(EDITOR_FAILURE_NONE),
            editor_width: AtomicU32::new(0),
            editor_height: AtomicU32::new(0),
            editor_resizable: AtomicU32::new(0),
            instance_id: instance_id.get(),
            editor_observed_open: AtomicBool::new(false),
            editor_close_generation: AtomicU64::new(0),
            editor_input: editor_surface::InputRing::new(),
        })
    }

    pub(crate) const fn prepared_block_frames(&self) -> u32 {
        self.block_frames
    }
}
