use super::{Package, linux};

#[test]
fn each_declared_linux_distribution_uses_its_qualified_package_format() {
    for distro in ["debian", "ubuntu", "kali"] {
        for arch in ["x64", "arm64"] {
            assert_eq!(
                linux(&format!("ID=\"{distro}\"\n"), arch).unwrap(),
                Package::Deb
            );
        }
    }
    assert_eq!(linux("ID='arch'\n", "x64").unwrap(), Package::Arch);
    for (release, arch) in [
        ("ID=arch", "arm64"),
        ("ID=fedora", "x64"),
        ("NAME=Debian", "x64"),
    ] {
        assert!(linux(release, arch).is_err());
    }
}

#[test]
fn package_selection_uses_url_path_and_never_offers_another_package_manager() {
    let arch = linux("ID=arch", "x64").unwrap();
    assert!(arch.accepts("https://downloads.example.test/mixrelay.pkg.tar.zst?release=1"));
    assert!(!arch.accepts("https://downloads.example.test/mixrelay.deb"));
    assert!(!arch.accepts("https://downloads.example.test/mixrelay.rpm"));
    assert!(!arch.accepts("https://downloads.example.test/mixrelay.deb?name=.pkg.tar.zst"));
    assert!(!arch.accepts("invalid-url"));
    assert!(Package::Pkg.accepts("https://downloads.example.test/universal.pkg"));
    assert!(!Package::Deb.accepts("https://downloads.example.test/universal.pkg"));
}

#[test]
fn the_support_contract_parses_with_non_installer_artifacts() {
    let support: serde_json::Value = serde_json::from_str(super::SUPPORT).unwrap();
    assert!(
        support["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|artifact| artifact["format"] == "streamDeckPlugin")
    );
    assert_eq!(linux("ID=ubuntu", "arm64").unwrap(), Package::Deb);
}
