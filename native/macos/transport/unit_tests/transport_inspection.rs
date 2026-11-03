use super::*;
use crate::endpoint_transport::{
    mr_endpoint_transport_initialize, mr_endpoint_transport_required_bytes,
};

struct Region {
    descriptor: OwnedFd,
    mapping: InspectionMapping,
    config: EndpointTransportConfig,
}

impl Region {
    fn new(channels: u32) -> Self {
        let config = EndpointTransportConfig {
            endpoint_id: [1; 16],
            capacity_frames: 16,
            render_channels: channels,
            capture_channels: channels,
        };
        let size = unsafe { mr_endpoint_transport_required_bytes(&raw const config) };
        let descriptor = crate::create_shared_memory(size, true).unwrap();
        let mapping = InspectionMapping::open(descriptor.as_raw_fd(), size).unwrap();
        assert!(
            !unsafe { mr_endpoint_transport_initialize(mapping.memory, size, &raw const config) }
                .is_null()
        );
        Self {
            descriptor,
            mapping,
            config,
        }
    }
}

#[test]
fn inspection_rejects_bad_stale_and_undersized_descriptors_without_taking_ownership() {
    let region = Region::new(2);
    let size = region.mapping.size;
    let actual_size = File::from(region.descriptor.try_clone().unwrap())
        .metadata()
        .unwrap()
        .len();
    for (raw, bytes) in [
        (-1, size),
        (c_int::MAX, size),
        (region.descriptor.as_raw_fd(), 0),
        (
            region.descriptor.as_raw_fd(),
            usize::try_from(actual_size).unwrap() + 1,
        ),
        (region.descriptor.as_raw_fd(), u32::MAX as usize + 1),
    ] {
        assert!(inspect_descriptor(raw, bytes).is_err());
    }
    assert!(region.descriptor.try_clone().is_ok());
    assert_eq!(
        inspect_descriptor(region.descriptor.as_raw_fd(), size).unwrap(),
        region.config
    );
}

#[test]
fn inspection_rejects_readonly_and_uninitialized_backing() {
    for writable in [false, true] {
        let descriptor = crate::create_shared_memory(4096, writable).unwrap();
        assert!(inspect_descriptor(descriptor.as_raw_fd(), 4096).is_err());
        assert!(descriptor.try_clone().is_ok());
    }
}

#[test]
fn inspection_accepts_current_discrete_channels_without_binding_or_clock_publication() {
    use crate::endpoint_transport::{EndpointClockSnapshot, mr_endpoint_transport_clock_snapshot};
    for channels in [1, 2, 7, 64] {
        let region = Region::new(channels);
        assert_eq!(
            inspect_descriptor(region.descriptor.as_raw_fd(), region.mapping.size).unwrap(),
            region.config
        );
        let mut clock = EndpointClockSnapshot::default();
        assert!(!unsafe {
            mr_endpoint_transport_clock_snapshot(region.mapping.memory.cast(), &raw mut clock)
        });
    }
}

#[test]
fn repeated_valid_and_rejected_inspection_releases_all_descriptors() {
    // Isolate FD numbering from other parallel Rust tests; this executes the
    // same test in a native Cargo child, not a skipped or alternate code path.
    if std::env::var_os("MR_INSPECTION_FD_TEST_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transport_inspection::tests::repeated_valid_and_rejected_inspection_releases_all_descriptors", "--test-threads=1"])
            .env("MR_INSPECTION_FD_TEST_CHILD", "1").status().unwrap();
        assert!(status.success());
        return;
    }
    let region = Region::new(2);
    let invalid = crate::create_shared_memory(4096, true).unwrap();
    let probe = region.descriptor.try_clone().unwrap();
    let expected = probe.as_raw_fd();
    drop(probe);
    for _ in 0..100 {
        assert_eq!(
            inspect_descriptor(region.descriptor.as_raw_fd(), region.mapping.size).unwrap(),
            region.config
        );
        assert!(inspect_descriptor(invalid.as_raw_fd(), 4096).is_err());
        let probe = region.descriptor.try_clone().unwrap();
        assert_eq!(
            probe.as_raw_fd(),
            expected,
            "inspection leaked a descriptor"
        );
    }
}
