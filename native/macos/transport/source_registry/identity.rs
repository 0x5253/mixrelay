//! Source identity and geometry admitted from a producer request, and the
//! opaque identifiers the registry hands out for it.
use crate::{application_clock::VERSION, source_control::SourceRequest};
use std::io;

/// Producer leases occupy the bits above the 16-bit shared-buffer epoch.
pub(super) const MAX_LEASE: u64 = (1 << 48) - 1;
const CAPACITY_FRAMES: std::ops::RangeInclusive<u32> = 1024..=1_048_576;
const CHANNELS: std::ops::RangeInclusive<u32> = 1..=64;

/// A validated request: a non-empty NUL-terminated ID and a supported geometry.
pub(super) struct SourceIdentity {
    pub(super) id: Vec<u8>,
    pub(super) token: u64,
    pub(super) capacity: u32,
    pub(super) channels: u32,
}

impl SourceIdentity {
    pub(super) fn validate(request: &SourceRequest) -> io::Result<Self> {
        let id = request
            .source_id
            .iter()
            .position(|byte| *byte == 0)
            .filter(|length| *length != 0)
            .map(|length| {
                request.source_id[..length]
                    .iter()
                    .map(|byte| byte.to_ne_bytes()[0])
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| io::Error::other("invalid source identity"))?;
        if request.transport_version != VERSION
            || !CAPACITY_FRAMES.contains(&request.capacity_frames)
            || !request.capacity_frames.is_power_of_two()
            || !CHANNELS.contains(&request.channels)
        {
            return Err(io::Error::other("invalid source format"));
        }
        Ok(Self {
            token: token(&id),
            id,
            capacity: request.capacity_frames,
            channels: request.channels,
        })
    }
}

/// FNV-1a of the source ID. Zero is reserved for "no source" in the C ABI.
pub(super) fn token(id: &[u8]) -> u64 {
    let hash = id
        .iter()
        .fold(14_695_981_039_346_656_037_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(1_099_511_628_211)
        });
    if hash == 0 { 1 } else { hash }
}

/// Opaque `SourceClock::epoch`: a lease above its 16-bit shared-buffer epoch.
pub(super) const fn stream_epoch(lease: u64, epoch: u64) -> u64 {
    (lease << 16) | epoch
}

/// Monotonic producer leases. A lease is spent only once its grant succeeded.
pub(super) struct Leases {
    next: u64,
}

impl Leases {
    pub(super) const fn new() -> Self {
        Self { next: 1 }
    }

    pub(super) fn peek(&self) -> io::Result<u64> {
        if self.next > MAX_LEASE {
            return Err(io::Error::other("source lease space exhausted"));
        }
        Ok(self.next)
    }

    pub(super) fn spend(&mut self) {
        self.next += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_char;

    fn request(id: &[u8], capacity_frames: u32, channels: u32) -> SourceRequest {
        let mut request = SourceRequest {
            capacity_frames,
            channels,
            transport_version: VERSION,
            ..SourceRequest::default()
        };
        for (target, byte) in request.source_id.iter_mut().zip(id) {
            *target = c_char::from_ne_bytes([*byte]);
        }
        request
    }

    #[test]
    fn admits_power_of_two_capacity_and_bounded_channels_with_the_id_token() {
        for (capacity, channels) in [(1024, 1), (1_048_576, 64), (4096, 2)] {
            let identity = SourceIdentity::validate(&request(b"app", capacity, channels)).unwrap();
            assert_eq!(identity.id, b"app");
            assert_eq!(identity.token, token(b"app"));
            assert_eq!((identity.capacity, identity.channels), (capacity, channels));
        }
    }

    #[test]
    fn rejects_unsupported_geometry_and_transport_version() {
        for (capacity, channels) in [(512, 2), (2_097_152, 2), (3072, 2), (1024, 0), (1024, 65)] {
            assert!(SourceIdentity::validate(&request(b"app", capacity, channels)).is_err());
        }
        let stale = SourceRequest {
            transport_version: VERSION - 1,
            ..request(b"app", 1024, 2)
        };
        assert!(SourceIdentity::validate(&stale).is_err());
    }

    #[test]
    fn rejects_empty_and_unterminated_ids() {
        assert!(SourceIdentity::validate(&request(b"", 1024, 2)).is_err());
        let unterminated = request(&[b'a'; 128], 1024, 2);
        assert!(SourceIdentity::validate(&unterminated).is_err());
    }

    #[test]
    fn stream_epoch_keeps_lease_and_buffer_epoch_separable() {
        let epoch = stream_epoch(MAX_LEASE, u64::from(u16::MAX));
        assert_eq!(epoch >> 16, MAX_LEASE);
        assert_eq!(epoch & u64::from(u16::MAX), u64::from(u16::MAX));
    }

    #[test]
    fn leases_advance_only_when_spent_and_stop_at_the_48_bit_limit() {
        let mut leases = Leases::new();
        assert_eq!(leases.peek().unwrap(), 1);
        assert_eq!(leases.peek().unwrap(), 1);
        leases.spend();
        assert_eq!(leases.peek().unwrap(), 2);
        let mut exhausted = Leases { next: MAX_LEASE };
        assert_eq!(exhausted.peek().unwrap(), MAX_LEASE);
        exhausted.spend();
        assert!(exhausted.peek().is_err());
    }
}
