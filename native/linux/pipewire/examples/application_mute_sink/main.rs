#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a lab tool, not shipped: setup failures abort with their message"
)]
#[cfg(target_os = "linux")]
mod callback_sink;
#[cfg(target_os = "linux")]
mod sink;
#[cfg(target_os = "linux")]
mod trace;

#[cfg(target_os = "linux")]
fn main() -> Result<(), sink::SinkError> {
    sink::run()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::FAILURE
}
