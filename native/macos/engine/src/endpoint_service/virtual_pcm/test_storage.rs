//! Test backing descriptors preserve the source mapping ABI without a global shm namespace.
pub(in super::super) fn create(bytes: usize) -> std::io::Result<std::os::fd::OwnedFd> {
    let directory = mixrelay_test_support::FixtureDir::new("vi-pcm")?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(directory.path().join("pcm"))?;
    file.set_len(u64::try_from(bytes).unwrap())?;
    Ok(file.into())
}

pub(in super::super) fn registry(
    clock: Box<dyn mixrelay_time::MonotonicClock>,
) -> mixrelay_macos_transport::source_registry::SourceRegistry {
    mixrelay_macos_transport::source_registry::SourceRegistry::with_clock_and_storage(
        clock,
        mixrelay_realtime::Timebase::current(),
        create,
    )
}
