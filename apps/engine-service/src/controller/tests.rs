//! The controller composed over real recorder sessions, processor state files and
//! document files, observed through its control API.
mod catalog_recording;
mod control_api;
mod controller_fixture;
mod driver_inventory;
mod execution_power;
mod identity;
mod node_recording;
mod recorder_capacity;
mod recorder_control;
mod staging;
mod workspace;

use super::*;
use controller_fixture::{ControllerFixture, settle};
use driver_inventory::{controller, controller_with_inventory, endpoint_document};
use mixrelay_audio_graph::{
    ChannelLayout, ExecutionPlan, GraphCompiler, NodeConfig, NodeSpec, StreamSpec,
    VirtualEndpointId, VirtualEndpointSpec,
};
use mixrelay_audio_io::{HostAudioBackendError, VirtualEndpointCapabilities};
use mixrelay_control_protocol::ErrorCode;
use mixrelay_control_server::ControlBackend;
use mixrelay_engine_core::{
    NoopGraphResourceBackend, PrepareProcessorError, ProcessorFactory, default_duplex_document,
    default_peak_meter_bank,
};
use mixrelay_engine_host::identity::new_incarnation;
use mixrelay_render::BlockProcessor;
