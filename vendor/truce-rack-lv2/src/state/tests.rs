use super::*;

fn encoded(properties: Vec<Property>) -> Vec<u8> {
    encoded_with_assets(properties, Vec::new())
}

fn encoded_with_assets(properties: Vec<Property>, assets: Vec<Asset>) -> Vec<u8> {
    serde_json::to_vec(&Snapshot {
        version: 1,
        plugin_uri: "urn:mixrelay:state-fixture".into(),
        byte_order: byte_order().into(),
        native_state: true,
        ports: BTreeMap::new(),
        properties,
        assets,
    })
    .unwrap()
}

fn take_callback_path(path: *mut std::ffi::c_char) -> String {
    assert!(!path.is_null());
    let value = unsafe { CStr::from_ptr(path) }.to_str().unwrap().to_owned();
    unsafe { assets::free_path(std::ptr::null_mut(), path) };
    value
}

fn fixture_opaque_property(snapshot: &Snapshot) -> &Property {
    snapshot
        .properties
        .iter()
        .find(|property| property.key == "urn:mixrelay:fixture:opaque")
        .expect("fixture opaque property")
}

fn fixture_opaque_property_mut(snapshot: &mut Snapshot) -> &mut Property {
    snapshot
        .properties
        .iter_mut()
        .find(|property| property.key == "urn:mixrelay:fixture:opaque")
        .expect("fixture opaque property")
}

#[test]
fn opaque_binary_property_is_owned_and_urid_values_remap_between_instances() {
    let first = UridMap::new();
    let key = map_uri(&first, "urn:fixture:opaque").unwrap();
    let value_type = map_uri(&first, "urn:fixture:binary").unwrap();
    let mut context = StoreContext {
        map: &first,
        properties: Vec::new(),
        bytes: 0,
        failure: None,
    };
    let mut bytes = [0_u8, 255, 13, 0];
    let status = unsafe {
        store(
            (&mut context as *mut StoreContext<'_>).cast(),
            key,
            bytes.as_ptr().cast(),
            bytes.len(),
            value_type,
            PORTABLE_POD,
        )
    };
    assert_eq!(status, 0);
    bytes.fill(99);
    let urid_key = map_uri(&first, "urn:fixture:selected").unwrap();
    let urid_type = map_uri(&first, &format!("{ATOM}URID")).unwrap();
    let urid = map_uri(&first, "urn:fixture:value").unwrap();
    assert_eq!(
        unsafe {
            store(
                (&mut context as *mut StoreContext<'_>).cast(),
                urid_key,
                (&urid as *const u32).cast(),
                4,
                urid_type,
                PORTABLE_POD,
            )
        },
        0
    );
    let encoded = encoded(context.properties);
    let second = UridMap::new();
    for id in 0..10 {
        map_uri(&second, &format!("urn:other:{id}")).unwrap();
    }
    let (_, restored) = decode(&encoded, "urn:mixrelay:state-fixture", &second).unwrap();
    assert_eq!(restored[0].bytes, [0, 255, 13, 0]);
    assert_ne!(restored[0].key, key);
    assert_eq!(
        u32::from_ne_bytes(restored[1].bytes.clone().try_into().unwrap()),
        map_uri(&second, "urn:fixture:value").unwrap()
    );
    let mut size = 0;
    let mut value_type = 0;
    let mut flags = 0;
    let raw = unsafe {
        retrieve(
            (&restored as *const Vec<RestoreProperty>).cast_mut().cast(),
            restored[0].key,
            &mut size,
            &mut value_type,
            &mut flags,
        )
    };
    assert_eq!(
        unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), size) },
        [0, 255, 13, 0]
    );
    assert_eq!(flags, PORTABLE_POD);
    assert_eq!(value_type, restored[0].value_type);
}

#[test]
fn invalid_nonportable_oversized_and_asset_properties_are_rejected() {
    let map = UridMap::new();
    let key = map_uri(&map, "urn:fixture:key").unwrap();
    let blob_type = map_uri(&map, "urn:fixture:blob").unwrap();
    let path_type = map_uri(&map, &format!("{ATOM}Path")).unwrap();
    let value = [1_u8; 4];
    for (value_type, flags, size) in [
        (blob_type, 1, 4),
        (blob_type, 3, PROPERTY_LIMIT + 1),
        (path_type, 3, 4),
        (blob_type, 3, 0),
    ] {
        let mut context = StoreContext {
            map: &map,
            properties: Vec::new(),
            bytes: 0,
            failure: None,
        };
        assert_ne!(
            unsafe {
                store(
                    (&mut context as *mut StoreContext<'_>).cast(),
                    key,
                    value.as_ptr().cast(),
                    size,
                    value_type,
                    flags,
                )
            },
            0
        );
        assert!(context.failure.is_some());
        assert!(context.properties.is_empty());
    }
    let malformed = encoded(vec![Property {
        key: "urn:fixture:key".into(),
        value_type: format!("{ATOM}Path"),
        value: PropertyValue::Bytes(b"/file\0".to_vec()),
    }]);
    assert!(decode(&malformed, "urn:mixrelay:state-fixture", &map).is_err());
    assert!(decode(&encoded(Vec::new()), "urn:different-plugin", &map).is_err());
    assert!(decode(b"broken", "urn:mixrelay:state-fixture", &map).is_err());
}

#[test]
fn path_property_requires_a_packaged_asset() {
    let map = UridMap::new();
    let encoded = encoded_with_assets(
        vec![Property {
            key: "urn:fixture:file".into(),
            value_type: format!("{ATOM}Path"),
            value: PropertyValue::Path("assets/0".into()),
        }],
        Vec::new(),
    );
    let failure = decode(&encoded, "urn:mixrelay:state-fixture", &map)
        .err()
        .expect("a path with no packaged asset must be rejected");
    assert!(failure.to_string().contains("missing asset"));
}

#[test]
#[cfg(unix)]
fn captured_file_asset_survives_fresh_restore_and_recapture_identically() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    use std::ffi::CString;

    let capture_root = tempfile::tempdir().unwrap();
    let mut capture = assets::SavePaths::new(capture_root.path(), None);
    let requested = CString::new("assets/preset.bin").unwrap();
    let made = unsafe {
        assets::save_make(
            (&mut capture as *mut assets::SavePaths).cast(),
            requested.as_ptr(),
        )
    };
    let source_path = take_callback_path(made);
    let original = b"host-private LV2 preset bytes\0\xff";
    std::fs::write(std::path::Path::new(&source_path), original).unwrap();

    let absolute = CString::new(source_path).unwrap();
    let abstract_path = unsafe {
        assets::save_abstract(
            (&mut capture as *mut assets::SavePaths).cast(),
            absolute.as_ptr(),
        )
    };
    let abstract_path = take_callback_path(abstract_path);
    assert_eq!(abstract_path, "assets/0");
    let same_abstract_path = unsafe {
        assets::save_abstract(
            (&mut capture as *mut assets::SavePaths).cast(),
            absolute.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(same_abstract_path), abstract_path);
    let captured = capture.package().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        STANDARD.decode(&captured[0].data).unwrap().as_slice(),
        original
    );
    assert_eq!(
        captured[0].sha256,
        format!("{:x}", Sha256::digest(original))
    );

    let fresh_root = assets::materialize(&captured).unwrap();
    let mut restore = assets::RestorePaths::new(fresh_root.path(), &captured);
    let abstract_path = CString::new(abstract_path).unwrap();
    let restored = unsafe {
        assets::restore_absolute(
            (&mut restore as *mut assets::RestorePaths).cast(),
            abstract_path.as_ptr(),
        )
    };
    let restored_path = take_callback_path(restored);
    assert_eq!(std::fs::read(&restored_path).unwrap().as_slice(), original);
    let restored_path_c = CString::new(restored_path.clone()).unwrap();
    let reversed_abstract = unsafe {
        assets::restore_abstract(
            (&mut restore as *mut assets::RestorePaths).cast(),
            restored_path_c.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(reversed_abstract), "assets/0");

    let recapture_root = tempfile::tempdir().unwrap();
    let mut recapture = assets::SavePaths::new(recapture_root.path(), Some(fresh_root.path()));
    let restored_path = CString::new(restored_path).unwrap();
    let recaptured_path = unsafe {
        assets::save_abstract(
            (&mut recapture as *mut assets::SavePaths).cast(),
            restored_path.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(recaptured_path), "assets/0");
    let recaptured = recapture.package().unwrap();
    assert_eq!(recaptured[0].data, captured[0].data);
    assert_eq!(recaptured[0].sha256, captured[0].sha256);
}

#[test]
fn tampered_asset_bytes_are_rejected_before_restore() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};

    let original = b"original asset";
    let tampered = Asset {
        path: "assets/0".into(),
        sha256: format!("{:x}", Sha256::digest(original)),
        data: STANDARD.encode(b"tampered asset"),
    };
    let failure = assets::materialize(&[tampered]).expect_err("a changed asset must be rejected");
    assert!(failure.to_string().contains("SHA-256 mismatch"));
}

#[test]
fn asset_size_accounting_is_exact_at_base64_padding_and_limit() {
    assert_eq!(assets::decoded_size("AA==").unwrap(), 1);
    assert_eq!(assets::decoded_size("AAA=").unwrap(), 2);
    assert_eq!(assets::decoded_size("AAAA").unwrap(), 3);
    assert!(assets::decoded_size("A===").is_err());
    assert_eq!(
        assets::add_asset_size(assets::MAX_ASSET_BYTES - 1, 1).unwrap(),
        assets::MAX_ASSET_BYTES
    );
    assert!(assets::add_asset_size(assets::MAX_ASSET_BYTES, 1).is_err());
}

#[test]
fn save_path_callbacks_reject_escape_and_symlink_sources() {
    use std::ffi::CString;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.bin");
    std::fs::write(&outside_file, b"outside").unwrap();
    let mut paths = assets::SavePaths::new(root.path(), None);
    let outside_path = CString::new(outside_file.to_str().unwrap()).unwrap();
    let mapped = unsafe {
        assets::save_abstract(
            (&mut paths as *mut assets::SavePaths).cast(),
            outside_path.as_ptr(),
        )
    };
    assert!(mapped.is_null());
    assert!(
        paths
            .failure
            .as_deref()
            .is_some_and(|message| message.contains("outside"))
    );

    let mut paths = assets::SavePaths::new(root.path(), None);
    let escape = CString::new("../outside.bin").unwrap();
    let made = unsafe {
        assets::save_make(
            (&mut paths as *mut assets::SavePaths).cast(),
            escape.as_ptr(),
        )
    };
    assert!(made.is_null());
    assert!(paths.failure.is_some());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        std::fs::create_dir(root.path().join("assets")).unwrap();
        let link = root.path().join("assets/link.bin");
        symlink(&outside_file, &link).unwrap();
        let mut paths = assets::SavePaths::new(root.path(), None);
        let link_path = CString::new(link.to_str().unwrap()).unwrap();
        let mapped = unsafe {
            assets::save_abstract(
                (&mut paths as *mut assets::SavePaths).cast(),
                link_path.as_ptr(),
            )
        };
        assert!(mapped.is_null());
        assert!(paths.failure.is_some());
    }
}

#[test]
#[cfg(unix)]
fn opened_asset_descriptor_is_close_on_exec() {
    use std::os::fd::AsRawFd;

    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("assets");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("capture.bin"), b"state").unwrap();
    let file =
        assets::open_regular(root.path(), std::path::Path::new("assets/capture.bin")).unwrap();
    // SAFETY: F_GETFD only reads flags for the still-owned descriptor.
    let descriptor_flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    assert!(descriptor_flags >= 0);
    assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
}

#[test]
fn restore_path_callback_rejects_unlisted_and_escaping_paths() {
    use std::ffi::CString;

    let root = tempfile::tempdir().unwrap();
    let known = [Asset {
        path: "assets/0".into(),
        sha256: String::new(),
        data: String::new(),
    }];
    for rejected in ["../outside.bin", "assets/1"] {
        let mut paths = assets::RestorePaths::new(root.path(), &known);
        let requested = CString::new(rejected).unwrap();
        let mapped = unsafe {
            assets::restore_absolute(
                (&mut paths as *mut assets::RestorePaths).cast(),
                requested.as_ptr(),
            )
        };
        assert!(mapped.is_null());
        assert!(paths.failure.is_some());
    }
}

#[test]
#[cfg(unix)]
fn save_absolute_callback_resolves_only_registered_files() {
    use std::ffi::CString;

    let save_root = tempfile::tempdir().unwrap();
    let save_assets = save_root.path().join("assets");
    std::fs::create_dir(&save_assets).unwrap();
    let save_file = save_assets.join("preset.bin");
    std::fs::write(&save_file, b"owned").unwrap();
    let mut save = assets::SavePaths::new(save_root.path(), None);
    let file_path = CString::new(save_file.to_str().unwrap()).unwrap();
    let mapped = unsafe {
        assets::save_abstract(
            (&mut save as *mut assets::SavePaths).cast(),
            file_path.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(mapped), "assets/0");

    let abstract_path = CString::new("assets/0").unwrap();
    let absolute = unsafe {
        assets::save_absolute(
            (&mut save as *mut assets::SavePaths).cast(),
            abstract_path.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(absolute), save_file.to_str().unwrap());

    let unknown = CString::new("assets/1").unwrap();
    let unmapped = unsafe {
        assets::save_absolute(
            (&mut save as *mut assets::SavePaths).cast(),
            unknown.as_ptr(),
        )
    };
    assert!(unmapped.is_null());
    assert!(save.failure.is_some());
}

#[test]
#[cfg(unix)]
fn restore_abstract_callback_resolves_only_listed_regular_files() {
    use std::{ffi::CString, os::unix::fs::symlink};

    let restore_root = tempfile::tempdir().unwrap();
    let restore_assets = restore_root.path().join("assets");
    std::fs::create_dir(&restore_assets).unwrap();
    let restore_file = restore_assets.join("0");
    std::fs::write(&restore_file, b"owned").unwrap();
    let known = [Asset {
        path: "assets/0".into(),
        sha256: String::new(),
        data: String::new(),
    }];
    let mut restore = assets::RestorePaths::new(restore_root.path(), &known);
    let absolute = CString::new(restore_file.to_str().unwrap()).unwrap();
    let abstract_path = unsafe {
        assets::restore_abstract(
            (&mut restore as *mut assets::RestorePaths).cast(),
            absolute.as_ptr(),
        )
    };
    assert_eq!(take_callback_path(abstract_path), "assets/0");

    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.bin");
    std::fs::write(&outside_file, b"outside").unwrap();
    let outside_path = CString::new(outside_file.to_str().unwrap()).unwrap();
    let rejected = unsafe {
        assets::restore_abstract(
            (&mut restore as *mut assets::RestorePaths).cast(),
            outside_path.as_ptr(),
        )
    };
    assert!(rejected.is_null());
    assert!(restore.failure.is_some());

    let escaped_path = restore_root.path().join("assets/../assets/0");
    let escaped_path = CString::new(escaped_path.to_str().unwrap()).unwrap();
    let rejected = unsafe {
        assets::restore_abstract(
            (&mut restore as *mut assets::RestorePaths).cast(),
            escaped_path.as_ptr(),
        )
    };
    assert!(rejected.is_null());
    assert!(restore.failure.is_some());

    let missing_root = tempfile::tempdir().unwrap();
    let mut missing = assets::RestorePaths::new(missing_root.path(), &known);
    let expected_file = missing_root.path().join("assets/0");
    let expected_path = CString::new(expected_file.to_str().unwrap()).unwrap();
    let rejected = unsafe {
        assets::restore_abstract(
            (&mut missing as *mut assets::RestorePaths).cast(),
            expected_path.as_ptr(),
        )
    };
    assert!(rejected.is_null());
    assert!(missing.failure.is_some());

    let symlink_root = tempfile::tempdir().unwrap();
    let symlink_assets = symlink_root.path().join("assets");
    std::fs::create_dir(&symlink_assets).unwrap();
    symlink(&outside_file, symlink_assets.join("0")).unwrap();
    let mut symlink_restore = assets::RestorePaths::new(symlink_root.path(), &known);
    let symlink_path = CString::new(symlink_assets.join("0").to_str().unwrap()).unwrap();
    let rejected = unsafe {
        assets::restore_abstract(
            (&mut symlink_restore as *mut assets::RestorePaths).cast(),
            symlink_path.as_ptr(),
        )
    };
    assert!(rejected.is_null());
    assert!(symlink_restore.failure.is_some());
}

/// Run only with the repository-owned fixture bundle compiled and `LV2_PATH` pointing to it.
/// This is an actual Lilv-loaded shared object, not a mock of the vendor state interface.
#[test]
#[ignore = "requires compiled fixtures/state.lv2 and isolated LV2_PATH"]
fn native_state_fixture_restores_opaque_bytes_and_ports_and_propagates_vendor_failures() {
    use truce_rack_core::{plugin::PluginCore, scanner::PluginScanner};
    let scanner = super::super::Lv2Scanner::new();
    let info = scanner
        .scan()
        .unwrap()
        .into_iter()
        .find(|info| info.unique_id == "urn:mixrelay:state-fixture")
        .expect("compile the repository-owned fixture and set isolated LV2_PATH");
    let mut plugin = scanner.load(&info).unwrap();
    plugin
        .activate(plugin.layouts[0].clone(), 48_000.0, 128)
        .unwrap();
    let original = plugin.save_state().unwrap();
    assert!(plugin.ui.controller.pending.publish(2, 0.625));
    let pending: Snapshot = serde_json::from_slice(&plugin.save_state().unwrap()).unwrap();
    assert_eq!(pending.ports["gain"].to_bits(), 0.625_f32.to_bits());
    assert_eq!(
        plugin.parameter_value(0).unwrap().to_bits(),
        0.625_f64.to_bits()
    );
    plugin.load_state(&original).unwrap();
    assert_eq!(plugin.ui.controller.pending.peek(0), None);
    assert_eq!(
        plugin.parameter_value(0).unwrap().to_bits(),
        1.0_f64.to_bits()
    );
    let mut snapshot: Snapshot = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        snapshot.properties.len(),
        2,
        "opaque native state and a mapped file path, not just control ports"
    );
    assert_eq!(snapshot.ports.len(), 1);
    assert_eq!(snapshot.assets.len(), 1);
    fixture_opaque_property_mut(&mut snapshot).value = PropertyValue::Bytes(vec![9, 8, 7, 0, 255]);
    snapshot.ports.insert("gain".into(), 0.25);
    plugin
        .load_state(&serde_json::to_vec(&snapshot).unwrap())
        .unwrap();
    let recalled: Snapshot = serde_json::from_slice(&plugin.save_state().unwrap()).unwrap();
    assert_eq!(recalled.ports["gain"].to_bits(), 0.25_f32.to_bits());
    assert!(
        matches!(&fixture_opaque_property(&recalled).value, PropertyValue::Bytes(bytes) if bytes == &[9,8,7,0,255])
    );
    let recalled_bytes = plugin.save_state().unwrap();
    // Changing a native Options bound/rate must rebuild the instance, not erase
    // its opaque preset or replace the shared control port with a default value.
    for (rate, block) in [(48_000.0, 256), (44_100.0, 256), (48_000.0, 128)] {
        plugin.deactivate();
        plugin
            .activate(plugin.layouts[0].clone(), rate, block)
            .unwrap();
        assert_eq!(plugin.save_state().unwrap(), recalled_bytes);
        assert_eq!(
            plugin.parameter_value(0).unwrap().to_bits(),
            0.25_f64.to_bits()
        );
    }
    // The fixture rejects a leading 254 on restore, and a leading 253 on subsequent save.
    fixture_opaque_property_mut(&mut snapshot).value = PropertyValue::Bytes(vec![254, 0, 0, 0, 0]);
    assert!(plugin.ui.controller.pending.publish(2, 0.375));
    assert!(
        plugin
            .load_state(&serde_json::to_vec(&snapshot).unwrap())
            .unwrap_err()
            .to_string()
            .contains("restore failed with status 1")
    );
    assert_eq!(
        plugin.parameter_value(0).unwrap().to_bits(),
        0.375_f64.to_bits()
    );
    fixture_opaque_property_mut(&mut snapshot).value = PropertyValue::Bytes(vec![253, 0, 0, 0, 0]);
    plugin
        .load_state(&serde_json::to_vec(&snapshot).unwrap())
        .unwrap();
    assert!(
        plugin
            .save_state()
            .unwrap_err()
            .to_string()
            .contains("save failed with status 1")
    );
}

/// Exercise the real LV2 makePath/mapPath/freePath callbacks through Lilv and a fresh instance.
#[test]
#[ignore = "requires compiled fixtures/state.lv2 and isolated LV2_PATH"]
fn native_state_fixture_restores_modified_file_asset_in_fresh_instance() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use sha2::{Digest, Sha256};
    use truce_rack_core::{plugin::PluginCore, scanner::PluginScanner};

    let scanner = super::super::Lv2Scanner::new();
    let info = scanner
        .scan()
        .unwrap()
        .into_iter()
        .find(|info| info.unique_id == "urn:mixrelay:state-fixture")
        .expect("compile the repository-owned fixture and set isolated LV2_PATH");
    let mut source = scanner.load(&info).unwrap();
    source
        .activate(source.layouts[0].clone(), 48_000.0, 128)
        .unwrap();
    let mut snapshot: Snapshot = serde_json::from_slice(&source.save_state().unwrap()).unwrap();
    assert_eq!(snapshot.assets.len(), 1);
    assert!(snapshot.properties.iter().any(|property| {
        property.key == "urn:mixrelay:fixture:file"
            && property.value_type == format!("{ATOM}Path")
            && matches!(&property.value, PropertyValue::Path(path) if path == &snapshot.assets[0].path)
    }));

    let modified = [31_u8, 47, 59, 71, 83];
    snapshot.assets[0].data = STANDARD.encode(modified.as_slice());
    snapshot.assets[0].sha256 = format!("{:x}", Sha256::digest(modified.as_slice()));
    let modified_state = serde_json::to_vec(&snapshot).unwrap();
    source.deactivate();
    drop(source);

    let mut fresh = scanner.load(&info).unwrap();
    fresh
        .activate(fresh.layouts[0].clone(), 48_000.0, 128)
        .unwrap();
    fresh.load_state(&modified_state).unwrap();
    let recaptured: Snapshot = serde_json::from_slice(&fresh.save_state().unwrap()).unwrap();
    assert_eq!(recaptured.assets.len(), 1);
    assert_eq!(
        STANDARD
            .decode(&recaptured.assets[0].data)
            .unwrap()
            .as_slice(),
        modified.as_slice()
    );
    assert_eq!(
        recaptured.assets[0].sha256,
        format!("{:x}", Sha256::digest(modified.as_slice()))
    );
    assert!(recaptured.properties.iter().any(|property| {
        property.key == "urn:mixrelay:fixture:file"
            && matches!(&property.value, PropertyValue::Path(path) if path == &recaptured.assets[0].path)
    }));
}
