//! Private-lab interposition at native buffer handoff, not a production transport.
#![cfg(target_os = "linux")]

mod directions;
mod journal;
mod record;

#[cfg(not(test))]
mod interpose;
