use super::*;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/multibus.lv2")
}

#[test]
fn exact_bundle_scan_retains_uri_identity_and_requested_path() {
    let path = fixture().canonicalize().unwrap();
    let infos = Lv2Scanner::new().scan_path(&path).unwrap();
    assert_eq!(infos.len(), 5);
    assert!(infos.iter().all(|info| info.path == path));
    assert!(infos.iter().all(|info| {
        info.format == FORMAT && info.unique_id.starts_with("urn:mixrelay:multibus-")
    }));
}

#[test]
fn unrelated_paths_are_rejected_before_native_discovery() {
    let bundle = fixture();
    for path in [
        bundle.join("missing.lv2"),
        bundle.join("manifest.ttl"),
        bundle.parent().unwrap().to_owned(),
    ] {
        assert!(Lv2Scanner::new().scan_path(&path).is_err());
    }
}

#[test]
fn exact_bundle_load_rejects_an_unlisted_uri_before_instantiation() {
    let mut info = Lv2Scanner::new().scan_path(&fixture()).unwrap().remove(0);
    info.unique_id = "urn:mixrelay:not-in-selected-bundle".to_owned();
    assert!(matches!(
        Lv2Plugin::load_from(&info),
        Err(Error::LoadFailed { reason, .. }) if reason.contains("no LV2 plugin matching URI")
    ));
}

#[test]
fn native_bundle_path_has_a_trailing_separator_without_lossy_aliases() {
    let path = Path::new("/private/selected % bundle.lv2");
    let encoded = path_to_cstring_with_trailing_sep(path).unwrap();
    assert_eq!(
        encoded.to_str().unwrap(),
        format!("{}{}", path.display(), std::path::MAIN_SEPARATOR)
    );
    assert!(path_to_cstring_with_trailing_sep(Path::new("/invalid\0.lv2")).is_err());
}
