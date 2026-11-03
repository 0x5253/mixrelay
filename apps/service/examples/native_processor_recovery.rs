//! Explicitly opted-in native CLAP crash/retry qualification with synthetic graph I/O.
//! No hardware, HTTP listener, user graph, default plug-in directory, or editor is opened.

#[path = "native_processor_recovery/dispatch.rs"]
mod dispatch;
#[path = "native_processor_recovery/harness.rs"]
mod harness;
#[path = "native_processor_recovery/qualification.rs"]
mod qualification;
#[path = "native_processor_recovery/resources.rs"]
mod resources;
#[path = "native_processor_recovery/worker.rs"]
mod worker;

fn main() {
    if let Err(error) = dispatch::run() {
        eprintln!("native processor recovery qualification failed: {error}");
        std::process::exit(1);
    }
}
