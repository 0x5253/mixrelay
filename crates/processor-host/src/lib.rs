//! Isolated native processor hosting and the control handles used by render-plan preparation.

use mixrelay_audio_graph::NodeId;
use std::time::Duration;

mod catalog;
mod control;
mod discovery;
mod dry_delay;
mod dsp_lifecycle;
mod editor_observation;
mod editor_surface;
mod factory;
mod files;
mod hosted;
mod instance_id;
mod latency;
mod mailbox_capture;
mod native_editor;
mod native_operation;
mod native_preparation;
mod preparation;
mod process_fault;
mod rack_formats;
mod reuse;
mod shared_transport;
mod state;
mod state_blobs;
mod state_capture;
mod supervision;
mod timing;
mod worker;
mod worker_configuration;
mod worker_paths;
mod worker_process;

pub use catalog::{CatalogNodeResolver, ProcessorCatalog};
pub use control::{ProcessorControlRegistry, ProcessorStateError};
pub use editor_observation::ProcessorEditorCloseObservation;
pub use factory::ExternalProcessorFactory;
pub use files::ProcessorFiles;
pub use latency::ProcessorLatencySnapshot;
pub use rack_formats::{
    PROBE_ARGUMENT as PLUGIN_PROBE_ARGUMENT, SCAN_ARGUMENT as PLUGIN_SCAN_ARGUMENT,
    WORKER_ARGUMENT as PLUGIN_WORKER_ARGUMENT,
    identity::{RackFormat, RackLocator},
    probe::run_probe as run_plugin_probe,
    scan::run_scan as run_plugin_scan,
};
pub use reuse::ProcessorReuseTransaction;
pub use state_blobs::{StateBlobReader, StateBlobWriter, StateBlobs, StateStorageError};
pub use worker::run_plugin_worker;

use control::HostedProcessorControl;

#[cfg(not(target_has_atomic = "64"))]
compile_error!("MixRelay processor shared transport requires native 64-bit atomics");

const WORKER_START_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum wait for one processor state response, not an entire graph's providers.
/// Each independently hosted processor receives its own bounded capture opportunity.
pub const PROCESSOR_STATE_TIMEOUT: Duration = Duration::from_secs(5);
const SHARED_MAGIC: u64 = 0x4d52_5653_5433_3031;
const SHARED_VERSION: u32 = 11;
const PARAMETER_CAPACITY: usize = 4096;
const STATE_CAPACITY: usize = mixrelay_processor_state::MAX_STATE_BYTES;
const STATE_OK: u32 = 0;
const STATE_FAILED: u32 = 1;
const STATE_TOO_LARGE: u32 = 2;
const STATE_UNSUPPORTED: u32 = 3;
const WORKER_STARTING: u32 = 0;
const WORKER_READY: u32 = 1;
const WORKER_FAILED: u32 = 2;
const EDITOR_COMMAND_NONE: u32 = 0;
const EDITOR_COMMAND_OPEN: u32 = 1;
const EDITOR_COMMAND_CLOSE: u32 = 2;
const EDITOR_STATE_UNAVAILABLE: u32 = 0;
const EDITOR_STATE_CLOSED: u32 = 1;
const EDITOR_STATE_OPENING: u32 = 2;
const EDITOR_STATE_OPEN: u32 = 3;
const EDITOR_STATE_FAILED: u32 = 4;
const EDITOR_FAILURE_NONE: u32 = 0;
const EDITOR_FAILURE_NO_EDITOR: u32 = 1;
const EDITOR_FAILURE_DISPLAY_UNAVAILABLE: u32 = 2;
const EDITOR_FAILURE_HOST_WINDOW: u32 = 3;
const EDITOR_FAILURE_PLUGIN_REJECTED: u32 = 4;
const EDITOR_FAILURE_WORKER_UNAVAILABLE: u32 = 5;

#[derive(Debug, Clone, Copy)]
pub(crate) struct AudioLayout {
    inputs: usize,
    outputs: usize,
    block_frames: usize,
}

#[cfg(test)]
mod tests;
