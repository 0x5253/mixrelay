use super::*;

fn account() -> ServiceAccount {
    ServiceAccount::new("listener".into(), "staff".into(), "/Users/listener".into()).unwrap()
}

fn template() -> Vec<u8> {
    include_bytes!("../../../../../packaging/macos/io.mixrelay.audio.driver.plist.in").to_vec()
}

#[test]
fn template_binds_account_and_roundtrips_with_driver_autostart() {
    let account = account();
    let definition = LaunchdConfiguration::from_template(&template(), &account).unwrap();
    let bytes = definition.to_xml().unwrap();
    assert_eq!(
        LaunchdConfiguration::validate_installed(&bytes, &account).unwrap(),
        definition
    );
    assert_eq!(definition.0.program_arguments, [DRIVER_PATH]);
    assert_eq!(
        definition.0.associated_bundle_identifiers,
        "io.mixrelay.desktop"
    );
    assert!(definition.0.run_at_load);
    assert!(definition.0.keep_alive);
    assert!(!String::from_utf8(bytes).unwrap().contains("__MIXRELAY_"));
}

#[test]
fn installed_account_is_extracted_only_from_the_fixed_closed_contract() {
    let account = account();
    let installed = LaunchdConfiguration::from_template(&template(), &account)
        .unwrap()
        .to_xml()
        .unwrap();
    assert_eq!(
        LaunchdConfiguration::installed_account(&installed).unwrap(),
        account
    );

    let mut value = plist::Value::from_reader(Cursor::new(installed)).unwrap();
    value
        .as_dictionary_mut()
        .unwrap()
        .insert("Program".into(), plist::Value::String("/bin/sh".into()));
    let mut tampered = Vec::new();
    value.to_writer_xml(&mut tampered).unwrap();
    assert!(LaunchdConfiguration::installed_account(&tampered).is_err());
}

#[test]
fn every_executable_or_lifecycle_policy_change_is_rejected() {
    let original = Definition::new(TEMPLATE_USER, TEMPLATE_GROUP, TEMPLATE_HOME);
    let changes: [fn(&mut Definition); 10] = [
        |value| value.program_arguments[0] = "/bin/sh".into(),
        |value| value.program_arguments.push("--extra".into()),
        |value| value.label = "other.service".into(),
        |value| value.associated_bundle_identifiers = "io.other.desktop".into(),
        |value| value.keep_alive = false,
        |value| value.run_at_load = false,
        |value| value.mach_services.driver = false,
        |value| value.user_name = "root".into(),
        |value| value.environment_variables.home = "/var/root".into(),
        |value| value.group_name = "wheel".into(),
    ];
    for change in changes {
        let mut modified = original.clone();
        change(&mut modified);
        assert!(matches!(
            LaunchdConfiguration::from_template(
                &LaunchdConfiguration(modified).to_xml().unwrap(),
                &account()
            ),
            Err(LaunchdError::Policy)
        ));
    }
}

#[test]
fn associated_bundle_identifier_must_be_the_exact_desktop_identity() {
    let original = String::from_utf8(template()).unwrap();
    for replacement in [
        "<string>io.other.desktop</string>",
        "<array><string>io.mixrelay.desktop</string></array>",
        "<integer>7</integer>",
    ] {
        let modified = original.replace(
            "<key>AssociatedBundleIdentifiers</key><string>io.mixrelay.desktop</string>",
            &format!("<key>AssociatedBundleIdentifiers</key>{replacement}"),
        );
        assert!(LaunchdConfiguration::from_template(modified.as_bytes(), &account()).is_err());
    }
}

#[test]
fn unknown_top_level_and_nested_keys_are_rejected() {
    for key in [
        "Program",
        "StartInterval",
        "Sockets",
        "WorkingDirectory",
        "StandardOutPath",
        "StandardErrorPath",
        "RootDirectory",
    ] {
        let mut value = plist::Value::from_reader(Cursor::new(template())).unwrap();
        value
            .as_dictionary_mut()
            .unwrap()
            .insert(key.into(), plist::Value::String("/bin/sh".into()));
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        assert!(LaunchdConfiguration::from_template(&bytes, &account()).is_err());
    }
    let text = String::from_utf8(template()).unwrap();
    for original in ["<key>HOME</key>", "<key>io.mixrelay.audio.driver</key>"] {
        let modified = text.replace(original, &format!("<key>EXTRA</key><true/>{original}"));
        assert!(LaunchdConfiguration::from_template(modified.as_bytes(), &account()).is_err());
    }
}

#[test]
fn duplicate_keys_and_wrong_field_types_are_rejected() {
    let text = String::from_utf8(template()).unwrap();
    let duplicate = text.replace(
        "<key>Label</key>",
        "<key>Label</key><string>other</string><key>Label</key>",
    );
    assert!(LaunchdConfiguration::from_template(duplicate.as_bytes(), &account()).is_err());
    let wrong_type = text.replace("<true/>", "<string>true</string>");
    assert!(LaunchdConfiguration::from_template(wrong_type.as_bytes(), &account()).is_err());
    assert!(matches!(
        LaunchdConfiguration::from_template(&vec![b' '; MAX_PLIST_BYTES + 1], &account()),
        Err(LaunchdError::TooLarge)
    ));
}

#[test]
fn account_values_are_escaped_and_cannot_inject_plist_or_shell_fields() {
    let account = ServiceAccount::new(
        "listener".into(),
        "staff".into(),
        "/Users/listener & <studio> $(echo nope)".into(),
    )
    .unwrap();
    let definition = LaunchdConfiguration::from_template(&template(), &account).unwrap();
    let bytes = definition.to_xml().unwrap();
    assert_eq!(
        LaunchdConfiguration::validate_installed(&bytes, &account).unwrap(),
        definition
    );
    assert!(String::from_utf8(bytes).unwrap().contains("&amp;"));
}

#[test]
fn escaped_output_cannot_exceed_the_configuration_read_budget() {
    let account = ServiceAccount::new(
        "listener".into(),
        "staff".into(),
        format!("/Users/{}", "&".repeat(4080)).into(),
    )
    .unwrap();
    let configuration = LaunchdConfiguration::from_template(&template(), &account).unwrap();
    assert!(matches!(
        configuration.to_xml(),
        Err(LaunchdError::TooLarge)
    ));
}

#[test]
fn invalid_accounts_and_mismatched_installed_identity_are_rejected() {
    for user in ["root", "", "..", "name with space", "x;id", "name\n"] {
        assert!(
            ServiceAccount::new(user.into(), "staff".into(), "/Users/listener".into()).is_err()
        );
    }
    for home in ["/", "relative", "/Users/../root", "/Users/name\n"] {
        assert!(ServiceAccount::new("listener".into(), "staff".into(), home.into()).is_err());
    }
    let definition = LaunchdConfiguration::from_template(&template(), &account()).unwrap();
    let other = ServiceAccount::new("other".into(), "staff".into(), "/Users/other".into()).unwrap();
    assert!(
        LaunchdConfiguration::validate_installed(&definition.to_xml().unwrap(), &other).is_err()
    );
}
