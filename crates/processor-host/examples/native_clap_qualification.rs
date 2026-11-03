//! Opt-in, device-free native CLAP worker qualification.
//!
//! Supply the explicitly built fixture, never a user plug-in directory:
//! `cargo run -p mixrelay-processor-host --example native_clap_qualification -- --run <fixture.clap>`
//! This exercises native loading, IPC, parameter delivery, state and route compensation.
//! Callbacks are deliberately paced for deterministic PCM; this is not a real-time benchmark.

#[path = "native_clap/dispatch.rs"]
mod dispatch;
#[path = "native_clap/graph.rs"]
mod graph;
#[path = "native_clap/multibus.rs"]
mod multibus;
#[path = "native_clap/qualification.rs"]
mod qualification;

fn main() -> Result<(), String> {
    dispatch::run()
}
