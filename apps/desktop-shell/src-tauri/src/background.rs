//! One user-owned controller keeps audio alive independently of editor windows.
mod command;
mod connection;
mod connection_failure;
mod directories;
mod dispatch;
#[cfg(test)]
mod fixture_pipe;
mod forward;
mod link;
mod process;
mod restore;
pub(crate) mod startup;

use connection::EXCHANGE_TIMEOUT;
pub(crate) use connection::{Connection, StartedTelemetrySubscription};
pub(crate) use connection_failure::ConnectionFailure;
pub(crate) use directories::{control_directory, data_directory};
pub(crate) use dispatch::dispatch;
pub(crate) use forward::forward;
pub(crate) use process::{ControllerLaunch, spawn_controller, spawn_editor};
pub(crate) use restore::{restore_captures, restore_driver};
