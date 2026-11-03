//! Emits the native catalog for standalone builds outside Cargo's `OUT_DIR`.
//! No source files are rewritten: the caller supplies its build output directory.
#[path = "../src/formats.rs"]
pub mod formats;
#[path = "../native_format_header.rs"]
mod native_format_header;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let directory = args.next().ok_or("expected one build output directory")?;
    if args.next().is_some() {
        return Err("expected one build output directory".into());
    }
    let directory = std::path::PathBuf::from(directory);
    native_format_header::write_header(&directory)?;
    Ok(())
}
