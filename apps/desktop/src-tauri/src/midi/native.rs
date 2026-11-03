//! `CoreMIDI` and ALSA sequencer adapters. The product opens inputs only.
use std::{collections::BTreeMap, sync::Arc};

use midir::{Ignore, MidiInput, MidiInputConnection};
use mixrelay_midi_control::{
    Device,
    learn::LearnCapture,
    mailbox::{InputSequence, Mailbox},
};

#[derive(Debug, thiserror::Error)]
pub(super) enum NativeError {
    #[error(
        "MIDI input backend unavailable (CoreMIDI on macOS; ALSA sequencer access on Linux): {0}"
    )]
    Init(#[from] midir::InitError),
    #[error("MIDI input port unavailable: {0}")]
    Port(#[from] midir::PortInfoError),
    #[error("MIDI input connection failed: {0}")]
    Connect(midir::ConnectErrorKind),
    #[error("MIDI input names are ambiguous; rename the duplicate client or port")]
    Ambiguous,
    #[cfg(target_os = "linux")]
    #[error("ALSA subscription discovery failed: {0}")]
    Alsa(#[from] alsa::Error),
}

pub(super) struct Port<C = MidiInputConnection<Arc<Mailbox>>> {
    pub(super) device: Device,
    pub(super) mailbox: Arc<Mailbox>,
    pub(super) native_id: String,
    pub(super) token: u32,
    _connection: C,
}

#[cfg(test)]
impl Port<()> {
    pub(super) fn fixture(device: Device, mailbox: Arc<Mailbox>) -> Self {
        Self {
            device,
            mailbox,
            native_id: "20:0".into(),
            token: 1,
            _connection: (),
        }
    }
}

type Discovered = Vec<(Device, String)>;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

fn connection_id(index: usize, id: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        macos::connection_id(index, id)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = index;
        id.to_owned()
    }
}

pub(super) fn reconcile(
    ports: &mut BTreeMap<String, Port>,
    capture: &Arc<LearnCapture>,
    next_token: &mut u32,
    sequence: &Arc<InputSequence>,
) -> Result<(), NativeError> {
    #[cfg(target_os = "linux")]
    let subscriptions = alsa::Seq::open(None, None, true).inspect_err(|_| ports.clear())?;
    let input = MidiInput::new("MixRelay discovery").inspect_err(|_| ports.clear())?;
    let native_ports = input.ports();
    let mut discovered = Vec::with_capacity(native_ports.len());
    let mut discovery_error = None;
    for (index, port) in native_ports.iter().enumerate() {
        let name = match input.port_name(port) {
            Ok(name) => name,
            Err(error) => {
                discovery_error = Some(NativeError::Port(error));
                continue;
            }
        };
        discovered.push((
            Device {
                id: identity(&port.id(), &name),
                name,
            },
            connection_id(index, &port.id()),
        ));
    }
    let result = reconcile_with(
        ports,
        discovered,
        |port| {
            #[cfg(target_os = "linux")]
            {
                linux::subscribed(&subscriptions, &port.native_id, &receiver_name(port.token))
            }
            #[cfg(target_os = "macos")]
            {
                let _ = port;
                true
            }
        },
        |device, native_id| {
            let native = native_ports
                .iter()
                .enumerate()
                .find(|(index, port)| connection_id(*index, &port.id()) == native_id)
                .map(|(_, port)| port)
                .ok_or(midir::PortInfoError::InvalidPort)?;
            *next_token = next_token.wrapping_add(1);
            let token = *next_token;
            open_port(device, native_id, native, (token, capture, sequence))
        },
    );
    discovery_error.map_or(result, Err)
}

fn open_port(
    device: Device,
    native_id: String,
    native: &midir::MidiInputPort,
    context: (u32, &Arc<LearnCapture>, &Arc<InputSequence>),
) -> Result<Port, NativeError> {
    let (token, capture, sequence) = context;
    let mut receiver = MidiInput::new(&receiver_name(token))?;
    receiver.ignore(Ignore::Sysex | Ignore::Time | Ignore::ActiveSense);
    let mailbox = Arc::new(Mailbox::new(Arc::clone(sequence)));
    let capture = Arc::clone(capture);
    let connection = receiver
        .connect(
            native,
            "MixRelay input",
            move |_, bytes, mailbox| {
                receive(token, bytes, mailbox, &capture, || {});
            },
            Arc::clone(&mailbox),
        )
        .map_err(|error| NativeError::Connect(error.kind()))?;
    Ok(Port {
        device,
        native_id,
        token,
        mailbox,
        _connection: connection,
    })
}

/// Reserve cancellation and receipt stamps before inspecting the learn state.
pub(super) fn receive(
    token: u32,
    bytes: &[u8],
    mailbox: &Mailbox,
    capture: &LearnCapture,
    before_active: impl FnOnce(),
) {
    let publication = mailbox.begin();
    before_active();
    if capture.active() {
        capture.offer(token, bytes);
        mailbox.suppress(bytes);
    } else {
        publication.receive(bytes);
    }
}

fn receiver_name(token: u32) -> String {
    format!("MixRelay controls {}:{token}", std::process::id())
}

/// Discovery and connection creation are separate so hot-plug policy runs with fake handles.
fn reconcile_with<C>(
    ports: &mut BTreeMap<String, Port<C>>,
    discovered: Discovered,
    mut valid: impl FnMut(&Port<C>) -> bool,
    mut open: impl FnMut(Device, String) -> Result<Port<C>, NativeError>,
) -> Result<(), NativeError> {
    let mut present = Vec::with_capacity(discovered.len());
    let mut error = None;
    for (device, native_id) in discovered {
        if present.contains(&device.id) {
            ports.remove(&device.id);
            error = Some(NativeError::Ambiguous);
            continue;
        }
        present.push(device.id.clone());
        if let Some(port) = ports.get_mut(&device.id)
            && port.native_id == native_id
            && valid(port)
        {
            port.device.name = device.name;
            continue;
        }
        ports.remove(&device.id);
        let id = device.id.clone();
        match open(device, native_id) {
            Ok(port) => {
                ports.insert(id, port);
            }
            Err(failed) => error = Some(failed),
        }
    }
    ports.retain(|id, _| present.contains(id));
    error.map_or(Ok(()), Err)
}

fn identity(id: &str, name: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        coremidi_identity(id, name)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = id;
        alsa_identity(name)
    }
}

#[cfg(any(target_os = "macos", test))]
fn coremidi_identity(id: &str, name: &str) -> String {
    if id == "0" {
        format!("coremidi-name:{name}")
    } else {
        format!("coremidi:{id}")
    }
}

#[cfg(any(target_os = "linux", test))]
fn alsa_identity(name: &str) -> String {
    // midir appends ephemeral ALSA client:port numbers after the two names.
    let names = name.rsplit_once(' ').map_or(name, |(names, _)| names);
    format!("alsa:{names}")
}

#[cfg(test)]
mod tests;
