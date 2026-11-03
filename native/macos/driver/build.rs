fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/MRDriverControl.c")
            .flag("-std=c17")
            .flag("-fblocks")
            .flag("-mmacosx-version-min=15.0")
            .warnings_into_errors(true)
            .compile("mixrelay_driver_control_native");
        println!("cargo:rerun-if-changed=src/MRDriverControl.c");
        println!("cargo:rustc-link-lib=framework=CoreAudio");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
