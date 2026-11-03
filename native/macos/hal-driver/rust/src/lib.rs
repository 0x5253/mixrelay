//! Multi-device `AudioServerPlugIn` state behind a narrow C ABI.
//!
//! The control plane owns descriptors, metadata and reclamation. Real-time
//! lookups use fixed slots and reader pins: removing a device cannot free a
//! runtime under a callback, and recycled slots receive new object generations.
#![cfg(target_os = "macos")]

mod abi;
mod clock;
mod control;
mod control_session;
mod control_signal;
mod device;
mod input_latency;
#[cfg(target_os = "macos")]
mod inventory;
mod io_admission;
mod io_dispatch;
mod io_outcome;
mod io_trace;
#[cfg(target_os = "macos")]
mod native;
mod output_control;
#[cfg(target_os = "macos")]
mod properties;
mod property_data;
mod property_notifications;
mod registry;
mod transport;

use mixrelay_audio_io::formats;

#[cfg(test)]
mod tests;
