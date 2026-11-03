//! macOS transport ABI and resource ownership shared by the HAL and engine.
//!
//! Owns native mappings, PCM projection and immutable device readback contracts.
//! Graph selection, admission planning and UI remain in their engine/app owners.
//! The static library is also used by native ABI contract tests without invoking
//! Cargo recursively from a build script.
#![cfg(target_os = "macos")]

pub mod aggregate_group;
mod anonymous_memory;
pub mod app_lifecycle;
pub mod application_clock;
pub(crate) mod application_source_mapping;
pub mod application_source_transport;
pub mod audio_buffer;
pub mod callback_clock;
pub mod capture_ring;
pub mod capture_selection;
pub mod client_pcm;
pub mod device_buffers;
pub mod device_io;
mod frame_times;
pub mod source_registry;
pub use anonymous_memory::create as create_shared_memory;
pub mod control_inventory;
pub mod endpoint_identity;
pub mod endpoint_transport;
pub mod hal_io;
pub mod shared_audio_buffer;
pub mod source_control;
pub mod transport_inspection;

/// Fixed application locations shared with the macOS installer.
pub mod install_contract {
    include!(concat!(env!("OUT_DIR"), "/product_paths.rs"));
}
