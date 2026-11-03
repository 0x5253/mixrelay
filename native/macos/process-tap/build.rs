fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let root = "../application-capture";
    cc::Build::new()
        .file(format!("{root}/src/MRProcessTap.m"))
        .include(format!("{root}/include"))
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=15.0")
        .warnings_into_errors(true)
        .compile("mixrelay_process_tap_objc");
    println!("cargo:rerun-if-changed={root}/src/MRProcessTap.m");
    println!("cargo:rerun-if-changed={root}/include");
    for framework in ["CoreAudio", "CoreFoundation", "Foundation", "AppKit"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
