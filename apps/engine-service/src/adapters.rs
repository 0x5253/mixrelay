//! Engine-core ports implemented over the processor host, the recorder registry and their
//! files. The controller composes them; nothing else depends on them.
pub(crate) mod processor_factory;
pub(crate) mod processor_ports;
pub(crate) mod recording_backend;
mod recording_sinks;
