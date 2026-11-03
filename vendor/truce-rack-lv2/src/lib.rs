//! LV2 host implementation for the truce-rack framework.
//!
//! Built on `lilv-sys` — the Rust FFI for **lilv**, the standard
//! LV2 host library. lilv handles URI resolution, TTL parsing, and
//! the Turtle world load that LV2 demands; truce-rack-lv2 is the layer
//! that turns those C-API queries into [`truce_rack_core::info::PluginInfo`] entries and
//! drives `lilv_plugin_instantiate` / `lilv_instance_run` from the
//! truce-rack-core trait surface.
//!
//! # Status
//!
//! - **Scan** via `lilv_world_load_all`, or one exact bundle via `lilv_world_load_bundle`.
//! - **Load** instantiates the plugin into a `LilvInstance` with
//!   the LV2 `urid#map` feature. Each plugin owns its own world
//!   (instance pointers reference world-owned data).
//! - **Process** connects audio / control / atom-sequence ports
//!   per block and calls `lilv_instance_run`. Audio ports point
//!   straight at the host's planes (zero-copy); control ports
//!   carry their declared default value; the MIDI input atom port
//!   is rebuilt each block from the truce-rack [`truce_rack_core::events::EventList`].
//! - **MIDI** in via atom sequence ports tagged `midi:MidiEvent`.
//!   Valid output MIDI events are copied into the host event list.
//!
//! # Build dependency
//!
//! `lilv-sys` links against the system `lilv-0` library. On macOS,
//! `brew install lilv`. On Debian/Ubuntu, `apt install liblilv-dev`.
//! Without those, `cargo build -p truce-rack-lv2` will fail at link
//! time.

#![allow(
    // Atom struct sizes are well under u32::MAX; the casts are for
    // FFI fields that are themselves u32.
    clippy::cast_possible_truncation,
    // `&x as *const _` reads cleaner here than `&raw const x`.
    clippy::borrow_as_ptr,
    clippy::ref_as_ptr,
    clippy::ptr_as_ptr
)]

mod atoms;
mod discovery;
mod editor;
mod features;
mod instance;
mod lifecycle;
mod options;
mod parameters;
mod pending_controls;
mod port_groups;
mod ports;
mod process;
mod state;

pub use discovery::Lv2Scanner;
pub use instance::Lv2Plugin;

/// Format identifier used on returned plugin metadata.
pub const FORMAT: &str = "lv2";

#[cfg(test)]
mod test_allocations;
