//! Build-time Rust projection of the shared macOS installation contract.

use std::{env, fmt::Write as _, fs, io, path::Path};

pub(crate) fn generate(header: &str) -> io::Result<()> {
    println!("cargo:rerun-if-changed={header}");
    println!("cargo:rerun-if-changed=../installer/product_paths.rs");
    let source = fs::read_to_string(header)?;
    let mut generated = String::new();
    for name in [
        "PRODUCT_DIRECTORY",
        "APP_PATH",
        "UNINSTALLER_PATH",
        "APP_EXECUTABLE_PATH",
        "APP_INFO_PATH",
        "REMOVAL_HELPER_PATH",
    ] {
        let path = expand(&source, &format!("MR_{name}"), 0)?;
        writeln!(
            generated,
            "/// Fixed path from `MRInstallContract.h`.\npub const {name}: &str = {path:?};"
        )
        .map_err(io::Error::other)?;
    }
    let output =
        env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("missing Cargo output directory"))?;
    fs::write(Path::new(&output).join("product_paths.rs"), generated)
}

fn expand(source: &str, name: &str, depth: usize) -> io::Result<String> {
    if depth > 8 {
        return Err(io::Error::other("recursive installation contract macro"));
    }
    let prefix = format!("#define {name} ");
    let mut expression = source
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .ok_or_else(|| io::Error::other(format!("missing installation contract macro: {name}")))?;
    let mut value = String::new();
    while !expression.trim().is_empty() {
        expression = expression.trim_start();
        if let Some(literal) = expression.strip_prefix('"') {
            let (text, remaining) = literal
                .split_once('"')
                .ok_or_else(|| io::Error::other("unterminated installation contract literal"))?;
            value.push_str(text);
            expression = remaining;
        } else {
            let end = expression
                .find(char::is_whitespace)
                .unwrap_or(expression.len());
            value.push_str(&expand(source, &expression[..end], depth + 1)?);
            expression = &expression[end..];
        }
    }
    Ok(value)
}
