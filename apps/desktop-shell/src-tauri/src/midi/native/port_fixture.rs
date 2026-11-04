//! Open input ports without a native connection, for tests outside the adapter.
use super::Port;
use mixrelay_midi_control::{Device, mailbox::Mailbox};
use std::sync::Arc;

/// An open input port without a native connection, delivering into `mailbox`.
pub(in crate::midi) fn port(device: Device, mailbox: Arc<Mailbox>) -> Port<()> {
    Port {
        device,
        mailbox,
        native_id: "20:0".into(),
        token: 1,
        _connection: (),
    }
}
