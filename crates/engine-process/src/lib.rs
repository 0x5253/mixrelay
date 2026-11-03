//! Private controller/engine process ownership, independent of audio processing.
//! The background controller retains a pipe writer; its closure (including controller death)
//! requests the engine's existing graceful drain. Readiness uses the child pipe,
//! never an arbitrary process listening on the public control port.

mod child;
mod parent;

pub use child::{EngineOwner, shutdown};
pub use parent::ControllerEngine;

/// Selects the private owned-child launch mode, not a public control command.
pub const CONTROLLER_ARGUMENT: &str = "--controller-owned";

/// Exact acknowledgement after the owned control listener is prepared.
/// Audio device and clock health remain the existing runtime status contract.
pub const READY: &[u8; 8] = b"MRREADY\n";
