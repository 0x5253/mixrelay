#![expect(
    clippy::expect_used,
    reason = "a build script fails the build by panicking; Cargo prints the message"
)]
use std::{
    env,
    path::{Path, PathBuf},
};

#[path = "../audio-io/src/formats.rs"]
pub mod formats;
#[path = "../audio-io/native_format_header.rs"]
mod native_format_header;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let root =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory")).join("../..");
    println!("cargo:rerun-if-changed=../../native/macos");
    println!("cargo:rerun-if-changed=../../native/pcm-ring");
    println!("cargo:rerun-if-changed=../audio-io/src/formats.rs");
    println!("cargo:rerun-if-changed=../audio-io/native_format_header.rs");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let libraries = FixtureLibraries {
        transport: library_receipt("MIXRELAY_NATIVE_TRANSPORT_STATICLIB"),
        hal: library_receipt("MIXRELAY_NATIVE_HAL_STATICLIB"),
    };
    native_format_header::write_header(&output).expect("generate native audio format declarations");
    build(
        &root,
        "clock",
        "native/macos/common/tests/MRCallbackClockTests.c",
        &libraries,
    );
    build(
        &root,
        "hal",
        "native/macos/hal-driver/tests/MRHALBridgeTests.c",
        &libraries,
    );
    build(
        &root,
        "application_clock",
        "native/macos/service/tests/MRApplicationClockTests.c",
        &libraries,
    );
    build(
        &root,
        "audio_ring",
        "native/macos/application-capture/tests/MRAudioRingTests.c",
        &libraries,
    );
    build(
        &root,
        "pcm_ring",
        "native/pcm-ring/tests/MRPcmRingTests.c",
        &libraries,
    );
    // Reports the callback timing layout exactly as the engine header declares it.
    let probe = output.join("device_io_timing_layout.c");
    std::fs::write(
        &probe,
        "#include <stdio.h>\n#include \"MRDeviceIO.h\"\n\
         int main(void) {\n\
             printf(\"%zu %zu %zu %zu\\n\", sizeof(MRIOTiming), offsetof(MRIOTiming, now),\n\
                 offsetof(MRIOTiming, input), offsetof(MRIOTiming, output));\n\
             return 0;\n\
         }\n",
    )
    .expect("write the timing layout probe");
    build(
        &root,
        "timing_layout",
        probe.to_str().expect("UTF-8 output directory"),
        &libraries,
    );
}

struct FixtureLibraries {
    transport: PathBuf,
    hal: PathBuf,
}

fn library_receipt(name: &str) -> PathBuf {
    println!("cargo:rerun-if-env-changed={name}");
    let path = env::var_os(name).map_or_else(
        || panic!("Missing {name}; use node scripts/lab/native-check.mjs cargo <command> to prepare HOST Cargo fixture libraries"),
        PathBuf::from,
    );
    let metadata = path.metadata().expect("read HOST Cargo staticlib receipt");
    assert!(
        path.is_absolute()
            && path.extension().is_some_and(|extension| extension == "a")
            && metadata.is_file()
            && metadata.len() > 0,
        "Invalid {name}; use the canonical native-check entry for HOST Cargo fixture libraries"
    );
    println!("cargo:rerun-if-changed={}", path.display());
    path
}

fn build(root: &Path, name: &str, test: &str, libraries: &FixtureLibraries) {
    let output_directory = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let output = output_directory.join(name);
    let compiler = cc::Build::new()
        .target(&env::var("HOST").expect("Cargo host"))
        .cargo_metadata(false)
        .get_compiler();
    let result = compiler
        .to_command()
        .current_dir(root)
        .arg("-I")
        .arg(&output_directory)
        .args([
            "-std=c17",
            "-fblocks",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-g",
            "-fsanitize=address,undefined",
            "-mmacosx-version-min=15.0",
        ])
        .args([
            "-I",
            "native/macos/service/include",
            "-I",
            "native/macos/hal-driver/include",
            "-I",
            "native/macos/hal-driver/rust/include",
            "-I",
            "native/macos/application-capture/include",
            "-I",
            "native/pcm-ring/include",
        ])
        .arg(test)
        // Each genuine Cargo staticlib already includes its Rust/native dependencies.
        // In particular HAL includes transport, PCM and the native XPC glue: link one archive.
        .arg(if name == "hal" {
            &libraries.hal
        } else {
            &libraries.transport
        })
        .arg("-liconv")
        .arg("-lobjc")
        .args([
            "-framework",
            "CoreAudio",
            "-framework",
            "CoreFoundation",
            "-framework",
            "AudioToolbox",
            "-o",
        ])
        .arg(&output)
        .output()
        .expect("launch native test compiler");
    assert!(
        result.status.success(),
        "{name} failed to compile:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    println!(
        "cargo:rustc-env=MR_NATIVE_{}={}",
        name.to_uppercase(),
        output.display()
    );
}
