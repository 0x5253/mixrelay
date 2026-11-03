#![expect(
    clippy::expect_used,
    reason = "a build script fails the build by panicking; Cargo prints the message"
)]
#[path = "../../../crates/audio-io/src/formats.rs"]
pub mod formats;
#[path = "../../../crates/audio-io/native_format_header.rs"]
mod native_format_header;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_engine();
    }
}

fn build_macos_engine() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    native_format_header::write_header(&output).expect("generate native audio format declarations");
    let engine_root = "../service";
    let hal_root = "../hal-driver";
    let engine_include = format!("{engine_root}/include");
    let hal_include = format!("{hal_root}/include");
    let sources = [format!("{engine_root}/src/MRService.c")];
    let mut build = cc::Build::new();
    build
        .include(&output)
        .include(&engine_include)
        .include("../../pcm-ring/include")
        .include(&hal_include)
        .flag("-std=c17")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=15.0")
        .warnings_into_errors(true)
        .files(&sources)
        .compile("mixrelay_service_native");

    for source in sources {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-changed={engine_include}/MREngineControl.h");
    println!("cargo:rerun-if-changed={engine_include}/MRApplicationClock.h");
    println!("cargo:rerun-if-changed={engine_include}/MRDeviceIO.h");
    println!("cargo:rerun-if-changed={engine_include}/MRAggregateGroup.h");
    println!("cargo:rerun-if-changed=../transport/MRCallbackClock.h");
    println!("cargo:rerun-if-changed=../transport/MRAnonymousMemory.h");
    println!("cargo:rerun-if-changed=../transport/MRDeviceBuffers.h");
    println!("cargo:rerun-if-changed=../../../crates/audio-io/src/formats.rs");
    println!("cargo:rerun-if-changed=../../../crates/audio-io/native_format_header.rs");
    println!("cargo:rerun-if-changed=../../pcm-ring/include/MRPcmRing.h");
    println!("cargo:rerun-if-changed={engine_include}/MREngineSourceRegistry.h");
    println!("cargo:rerun-if-changed={hal_include}/MRHALControlProtocol.h");
    println!("cargo:rerun-if-changed={hal_include}/MREndpointTransport.h");
    println!("cargo:rerun-if-changed={hal_include}/MRSharedAudioBuffer.h");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    cc::Build::new()
        .file(format!("{engine_root}/src/MRMicrophonePermission.m"))
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=15.0")
        .warnings_into_errors(true)
        .compile("mixrelay_microphone_permission");
    println!("cargo:rerun-if-changed={engine_root}/src/MRMicrophonePermission.m");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Security");
}
