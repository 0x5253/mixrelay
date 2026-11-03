#[path = "../installer/product_paths.rs"]
mod product_paths;

fn main() -> std::io::Result<()> {
    product_paths::generate("../installer/MRInstallContract.h")?;
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Ok(());
    }
    // XPC's blocks-based connection handler is the only producer transport C glue.
    cc::Build::new()
        .file("../application-capture/src/MRApplicationSourceTransport.c")
        .include("../hal-driver/include")
        .flag("-std=c17")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=15.0")
        .warnings_into_errors(true)
        .compile("mixrelay_source_xpc");
    println!("cargo:rerun-if-changed=../application-capture/src/MRApplicationSourceTransport.c");
    println!("cargo:rerun-if-changed=../hal-driver/include/MRHALControlProtocol.h");
    Ok(())
}
