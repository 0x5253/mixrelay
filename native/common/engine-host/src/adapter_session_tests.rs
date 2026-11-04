//! The adapter runtime status that both native services share, driven by a real
//! `AudioWorker` thread and file-backed session preferences.
mod external_rate;
mod format_catalog;
mod routing;
mod status;

use crate::{AudioWorker, session_preferences::SessionPreferences};
use mixrelay_audio_graph::RenderFormat;
use mixrelay_audio_io::session::{
    AudioBackendKind, AudioSessionCapabilities, ClockAuthority, SessionFormat, SessionIssue,
};
use mixrelay_control_protocol::DriverState;
use mixrelay_engine_core::session::*;
use mixrelay_engine_core::{
    ProcessingControlBackend, RuntimeBackend, RuntimeStatusBackend, SessionFormatBackend,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

fn profile(backend: AudioBackendKind) -> AdapterProfile {
    AdapterProfile {
        initial_format: SessionFormat {
            sample_rate: 44_100,
            block_frames: 128,
        },
        capabilities: AudioSessionCapabilities {
            backend,
            supported_sample_rates: vec![44_100, 48_000, 96_000],
            supported_block_frames: vec![64, 128, 256],
            can_reconfigure_live: false,
            hardware_input: true,
            hardware_output: false,
            virtual_endpoint: true,
            application_capture: true,
            sample_rate_conversion: false,
            drift_correction: false,
        },
        application_capture_rates: vec![44_100, 96_000],
        clock_authority: ClockAuthority::AudioServer,
        transport_latency_frames: None,
        requires_observed_device_rate: false,
    }
}
