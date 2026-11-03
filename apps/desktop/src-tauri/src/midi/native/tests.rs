use super::*;

fn discovered(address: &str) -> Discovered {
    vec![(
        Device {
            id: "stable".into(),
            name: "Controller".into(),
        },
        address.into(),
    )]
}
fn open(device: Device, native_id: String) -> Result<Port<()>, NativeError> {
    if native_id == "unavailable" {
        return Err(NativeError::Port(midir::PortInfoError::InvalidPort));
    }
    Ok(Port {
        device,
        native_id,
        mailbox: Arc::default(),
        token: 1,
        _connection: (),
    })
}
#[test]
fn hotplug_preserves_connections_and_reconnects_by_stable_identity_after_address_change() {
    let mut ports = BTreeMap::new();
    reconcile_with(&mut ports, discovered("20:0"), |_| true, open).unwrap();
    let original = Arc::clone(&ports["stable"].mailbox);
    reconcile_with(
        &mut ports,
        discovered("20:0"),
        |_| true,
        |_, _| panic!("unchanged port must not reopen"),
    )
    .unwrap();
    assert!(Arc::ptr_eq(&original, &ports["stable"].mailbox));
    reconcile_with(&mut ports, vec![], |_| true, open).unwrap();
    assert!(ports.is_empty());
    reconcile_with(&mut ports, discovered("25:0"), |_| true, open).unwrap();
    assert_eq!(ports["stable"].native_id, "25:0");
    assert!(!Arc::ptr_eq(&original, &ports["stable"].mailbox));
    reconcile_with(&mut ports, discovered("26:0"), |_| true, open).unwrap();
    assert_eq!(ports["stable"].native_id, "26:0");
}
#[test]
fn ambiguous_names_and_connection_failures_are_visible_without_deleting_preferences() {
    let mut ports = BTreeMap::new();
    let mut duplicate = discovered("20:0");
    duplicate.extend(discovered("21:0"));
    assert!(matches!(
        reconcile_with(&mut ports, duplicate, |_| true, open),
        Err(NativeError::Ambiguous)
    ));
    assert!(reconcile_with::<()>(&mut ports, discovered("unavailable"), |_| true, open).is_err());
    reconcile_with(&mut ports, discovered("31:0"), |_| true, open).unwrap();
    assert_eq!(ports["stable"].native_id, "31:0");
}
#[test]
fn coremidi_identity_uses_unique_id_and_falls_back_only_when_the_id_is_unavailable() {
    assert_eq!(coremidi_identity("42", "renamed"), "coremidi:42");
    assert_eq!(
        coremidi_identity("0", "Controller"),
        "coremidi-name:Controller"
    );
}
#[test]
fn alsa_identity_keeps_client_and_port_names_but_not_ephemeral_addresses() {
    assert_eq!(alsa_identity("client:port"), "alsa:client:port");
    assert_eq!(
        alsa_identity("Studio Controller:Port One 20:0"),
        alsa_identity("Studio Controller:Port One 25:1")
    );
}

#[test]
fn one_unavailable_input_does_not_disconnect_other_controllers_or_keep_stale_ports() {
    let mut ports = BTreeMap::new();
    reconcile_with(&mut ports, discovered("20:0"), |_| true, open).unwrap();
    let original = Arc::clone(&ports["stable"].mailbox);
    let mut inputs = discovered("20:0");
    inputs.push((
        Device {
            id: "busy".into(),
            name: "Busy controller".into(),
        },
        "unavailable".into(),
    ));
    assert!(reconcile_with(&mut ports, inputs, |_| true, open).is_err());
    assert_eq!(ports.len(), 1);
    assert!(Arc::ptr_eq(&original, &ports["stable"].mailbox));
    let mut duplicate = discovered("20:0");
    duplicate.extend(discovered("21:0"));
    assert!(reconcile_with(&mut ports, duplicate, |_| true, open).is_err());
    assert!(ports.is_empty());
}

#[test]
fn a_disconnected_subscription_reopens_even_if_name_and_address_are_reused_between_scans() {
    let mut ports = BTreeMap::new();
    reconcile_with(&mut ports, discovered("20:0"), |_| true, open).unwrap();
    let original = Arc::clone(&ports["stable"].mailbox);
    reconcile_with(&mut ports, discovered("20:0"), |_| false, open).unwrap();
    assert!(!Arc::ptr_eq(&original, &ports["stable"].mailbox));
    ports["stable"].mailbox.receive(&[0x90, 60, 100]);
    assert_eq!(
        ports["stable"]
            .mailbox
            .take(0, mixrelay_midi_control::Kind::Note, 60)
            .unwrap()
            .presses,
        1
    );
}
