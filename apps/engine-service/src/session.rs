//! Native session ownership assembled with engine runtime policy.
#[cfg(test)]
use crate::{ProcessingControlBackend, RuntimeStatusBackend, SessionFormatBackend};
#[cfg(test)]
use mixrelay_audio_graph::RenderFormat;
#[cfg(test)]
use mixrelay_audio_io::session::{
    AudioSessionCapabilities, ClockAuthority, SessionFormat, SessionIssue,
};
#[cfg(test)]
use mixrelay_control_protocol::DriverState;
use mixrelay_engine_core::session::*;
use mixrelay_engine_host::AudioWorker;
#[cfg(test)]
use mixrelay_engine_host::session_preferences::SessionPreferences;
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
#[cfg(test)]
#[path = "session_catalog_tests.rs"]
mod catalog_tests;
#[cfg(test)]
#[path = "session_native_format_tests.rs"]
mod native_format_tests;
#[cfg(test)]
#[path = "session_rate_tests.rs"]
mod rate_tests;
#[cfg(test)]
#[path = "session_routing_tests.rs"]
mod routing_tests;
#[cfg(test)]
#[path = "session/tests.rs"]
mod tests;

use std::time::Duration;
