//! Versioned XPC inventory codec. This module owns message/descriptor lifetimes,
//! not the Driver service or HAL registry. A failed snapshot never publishes a partial
//! authoritative list, so malformed traffic cannot trigger device removal.
#![cfg(target_os = "macos")]

mod endpoint_requests;
mod latency;
mod policy;
pub mod report;
mod snapshot;
#[cfg(test)]
mod tests;
mod xpc;

pub use endpoint_requests::{
    mr_hal_control_create_endpoint_remove, mr_hal_control_create_transport_replacement_request,
    mr_hal_control_parse_endpoint_remove, mr_hal_control_parse_transport_replacement_request,
};
pub use latency::{InputLatency, parse_input_latencies, write_input_latencies};
pub use policy::{ProducerPolicy, TransportCompatibility};
pub use snapshot::{
    InventoryEntry, InventorySnapshot, create_inventory_snapshot,
    mr_hal_control_create_inventory_request, mr_hal_control_create_inventory_snapshot,
    mr_hal_control_create_inventory_snapshot_reply, mr_hal_control_inventory_snapshot_close,
    mr_hal_control_parse_inventory_request, mr_hal_control_parse_inventory_snapshot,
};
pub(crate) use xpc::{
    Message, Xpc, new_message, operation, uint, xpc_dictionary_create_reply, xpc_dictionary_dup_fd,
    xpc_dictionary_set_fd, xpc_dictionary_set_string, xpc_dictionary_set_uint64,
};

pub const MAX_ENTRIES: usize = 64;
pub const NAME_BYTES: usize = 128;
pub const MAX_TRANSPORT_BUNDLES: usize = 32;
