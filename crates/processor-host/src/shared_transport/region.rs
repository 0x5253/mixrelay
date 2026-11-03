//! Owned byte regions consumed by the shared processor protocol.

/// Stable, initialized bytes whose lifetime includes every protocol view.
/// # Safety
/// The base pointers must address the same live allocation for `byte_length` bytes and
/// remain stable until Drop. Shared access must permit mutation through those pointers;
/// implementations must never create a mutable borrow spanning live protocol regions.
pub(super) unsafe trait TransportRegion: Send + Sync {
    fn byte_length(&self) -> usize;
    fn as_ptr(&self) -> *const u8;
    fn as_mut_ptr(&self) -> *mut u8;
}

/// A prepared nonblocking wake after a parent sequence advances.
pub(super) trait WorkerWake: Send + Sync {
    fn signal(&self, word: &std::sync::atomic::AtomicU64);
}

#[derive(Debug, thiserror::Error)]
pub(super) enum TransportLayoutError {
    #[error("shared processor sample count overflowed")]
    SampleCount,
    #[error("shared processor byte count overflowed")]
    ByteCount,
    #[error("shared processor mapping size overflowed")]
    MappingSize,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum TransportMemoryError {
    #[error("shared processor mapping is truncated or misaligned")]
    TruncatedOrMisaligned,
    #[error("shared processor mapping has an incompatible header")]
    IncompatibleHeader,
    #[error("shared processor mapping has a zero instance identifier")]
    ZeroInstance,
    #[error("shared processor mapping has an invalid size")]
    InvalidSize,
    #[error(transparent)]
    Layout(#[from] TransportLayoutError),
}
