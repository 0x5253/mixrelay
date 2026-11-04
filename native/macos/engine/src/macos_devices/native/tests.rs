use super::Native;
use crate::macos_devices::{
    device_fixture::synthetic_output, simulated::SimulatedHost, spec::Spec,
};
use mixrelay_audio_graph::DeviceSelector;
use std::sync::Arc;

#[test]
fn virtual_device_buffer_follows_the_session_block() {
    let driver = Native::create(
        &Spec {
            selector: DeviceSelector::PersistentUid {
                uid: mixrelay_macos_transport::endpoint_identity::endpoint_uid([1; 16]),
            },
            input: false,
            channels: 2,
        },
        48_000,
        None,
        32,
        SimulatedHost::new(),
    )
    .unwrap();
    assert_eq!(driver.requested_buffer(), 32);
    driver.request_buffer(256);
    assert_eq!(driver.requested_buffer(), 256);
}

#[test]
fn native_owner_allocation_retirement_stress() {
    // Lazily registered diagnostics callsites allocate once, outside the measurement.
    drop(synthetic_output(1, 48_000, 48_000));
    // All 260 owners exceed the 256-owner admission bound: every create below succeeds
    // only because each retirement returned its slot. Wide owners cost proportionally more
    // queue storage, so most cycles use one channel and each width still cycles twice.
    // Allocation is counted on this one thread, which creates and destroys every owner, so
    // concurrent tests cannot perturb it.
    let allocation = allocation_counter::measure(|| {
        for (channels, cycles) in [(1, 254), (2, 2), (8, 2), (64, 2)] {
            for _ in 0..cycles {
                let (device, context) = synthetic_output(channels, 48_000, 48_000);
                let native = Arc::downgrade(&device.native);
                drop(device);
                assert!(
                    native.upgrade().is_some(),
                    "worker still owns native queues"
                );
                drop(context);
                assert!(
                    native.upgrade().is_none(),
                    "last owner releases both queues and admission"
                );
            }
        }
    });
    assert!(allocation.count_total > 0, "owners allocate their queues");
    assert_eq!(
        (allocation.count_current, allocation.bytes_current),
        (0, 0),
        "native create/retire cycles leaked"
    );
}
