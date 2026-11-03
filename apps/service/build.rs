#![expect(
    clippy::expect_used,
    reason = "a build script fails the build by panicking; Cargo prints the message"
)]
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        embed_service_info_plist();
    }
}

fn embed_service_info_plist() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"),
    );
    let info_plist = manifest_dir
        .join("../../native/macos/service/Info.plist")
        .canonicalize()
        .expect("resolve the engine privacy Info.plist");
    let info_plist_argument = service_info_plist_link_argument(&info_plist);
    println!("cargo:rerun-if-changed={}", info_plist.display());
    println!("{info_plist_argument}");
}

fn service_info_plist_link_argument(info_plist: &std::path::Path) -> String {
    format!(
        "cargo:rustc-link-arg-bin=mixrelay-service=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        info_plist.display()
    )
}

#[cfg(test)]
mod tests {
    use super::service_info_plist_link_argument;
    use std::path::Path;

    #[test]
    fn privacy_plist_link_arg_targets_only_the_service_binary() {
        let argument = service_info_plist_link_argument(Path::new("/repo/native/Info.plist"));
        assert!(argument.starts_with(
            "cargo:rustc-link-arg-bin=mixrelay-service=-Wl,-sectcreate,__TEXT,__info_plist,"
        ));
        assert!(argument.ends_with("/repo/native/Info.plist"));
    }
}
