//! PipeWire-native transport and control-side connection ownership.
//! Contexts use the installed audio-client configuration, without changing daemon
//! metadata, process environment or OS scheduling permissions. Stream callbacks
//! must drain before their parent connection is released.
#![cfg(target_os = "linux")]

use mixrelay_pcm_ring as _;

mod capture_control;
mod connection;
mod connection_ffi;
mod control_time;
mod driver;
mod driver_ffi;
mod stream;
pub use connection::{Connection, ConnectionError, ConnectionHandles};
pub use driver::{
    DriverCycle, DriverError, DriverFormat, DriverProcess, DriverProcessFailure, GraphDriver,
    GraphInputBinding, GraphInputs,
};
pub use stream::{
    DescriptionChange, NativeStream, OutputRelease, StreamFormat, StreamOwners, StreamProperties,
    StreamRole, native_sample_format,
};
#[cfg(any(test, feature = "test-observation"))]
pub use stream::{
    NativeTransferSnapshot, RejectedTransfer, TRANSFER_SNAPSHOT_WORDS, TransferSnapshot,
};
