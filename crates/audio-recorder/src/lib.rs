//! Bounded realtime capture into a control-owned RF64 recording session.
//!
//! A registry outlives its render plans. Preparing or dropping a plan never closes
//! a take: only a control command, reaping after all taps retire, or registry
//! shutdown drains/finalizes its writer. No file or thread handle is held by a tap.
mod disk_files;
mod recording_failure;
mod recording_files;
mod registry;
mod sessions;
mod threaded_recording;
mod transport;
mod wave;
mod worker;

pub use recording_failure::{AdmissionFailure, RecorderError, RecorderFailure};
pub use registry::RecorderRegistry;
pub use transport::RecorderTap;

use mixrelay_audio_io::recording::{
    Encoding, RecorderAction, RecorderRequest, RecorderState, RecorderStatus,
};

fn valid_rate(rate: u32) -> bool {
    mixrelay_audio_io::formats::SAMPLE_RATES.contains(&rate)
}

#[cfg(test)]
mod tests;
