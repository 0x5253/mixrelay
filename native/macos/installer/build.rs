mod product_paths;

fn main() -> std::io::Result<()> {
    product_paths::generate("MRInstallContract.h")?;
    println!("cargo:rerun-if-changed=src/removal/native.m");
    println!("cargo:rerun-if-changed=src/removal/MRRemovalSession.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/removal/native.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .flag("-Werror")
            .compile("mixrelay_removal_authorization");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Security");
    }
    Ok(())
}
