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
    for arch in ["x64", "arm64"] {
        assert_eq!(linux("ID=fedora\n", arch).unwrap(), Package::Rpm);
    }
    for (release, arch) in [
        ("ID=arch", "arm64"),
        ("ID=opensuse-tumbleweed", "x64"),
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
    assert!(Package::Rpm.accepts("https://downloads.example.test/MixRelay-1.0.0-1.x86_64.rpm"));
    assert!(!Package::Rpm.accepts("https://downloads.example.test/MixRelay_1.0.0_amd64.deb"));
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

#[test]
fn release_assets_name_every_contract_artifact_at_the_version() {
    let assets = super::release_assets("1.2.3").unwrap();
    let installer = |name: &str| {
        assets
            .iter()
            .find(|asset| asset.name == name)
            .map(|asset| (asset.os.as_str(), asset.arch.as_str(), asset.installer))
    };
    assert_eq!(
        installer("MixRelay-Installer-1.2.3.pkg"),
        Some(("macos", "universal", true))
    );
    assert_eq!(
        installer("MixRelay-1.2.3-1.aarch64.rpm"),
        Some(("linux", "arm64", true))
    );
    assert_eq!(
        installer("MixRelay-StreamDeck-1.2.3.streamDeckPlugin"),
        Some(("macos", "universal", false))
    );
    assert!(assets.iter().all(|asset| !asset.name.contains("{version}")));
}
