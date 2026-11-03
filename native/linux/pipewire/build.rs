#![expect(
    clippy::expect_used,
    reason = "a build script fails the build by panicking; Cargo prints the message"
)]
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let library = pkg_config::Config::new()
        .atleast_version("1.2.0")
        .probe("libpipewire-0.3")
        .expect("install PipeWire 1.2 or newer and its development package (libpipewire-0.3-dev / pipewire)");
    println!("cargo:rerun-if-env-changed=MIXRELAY_LAB_PROCESS_OBSERVER");
    let mut build = cc::Build::new();
    if std::env::var("MIXRELAY_LAB_PROCESS_OBSERVER").as_deref() == Ok("1") {
        build.define("MIXRELAY_LAB_PROCESS_OBSERVER", None);
    }
    build
        .includes(library.include_paths)
        .include(".")
        .include("../../pcm-ring/include")
        .flag("-std=c17")
        .define("_GNU_SOURCE", None)
        .warnings_into_errors(true)
        .file("MRPipeWire.c")
        .compile("mixrelay_pipewire_native");
    for path in [
        "MRPipeWire.c",
        "MRPipeWire.h",
        "MRPipeWireConnection.h",
        "../../pcm-ring/include/MRPcmRing.h",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}
