//! IO-thread property reads: refused when they need control, never allocating otherwise.
use crate::abi::OK;
use crate::properties::{Address, ffi::mr_hal_core_property_get};
use crate::tests::Fixture;

#[test]
fn io_threads_are_refused_snapshot_properties_instead_of_waiting_for_control() {
    use std::sync::mpsc;
    use std::time::Duration;

    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = &*fixture.registry;
    let device = registry.pin(id).unwrap();
    let control = crate::device::lock(&device.control);
    let (done, finished) = mpsc::channel();
    let timely = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _audio = crate::transport::AudioScope::enter();
            let registry = (&raw const *registry).cast_mut();
            let read = |selector: [u8; 4], scope: u32| {
                let address = Address {
                    selector: u32::from_be_bytes(selector),
                    scope,
                    element: 0,
                };
                let mut value = [0_u8; 64];
                let mut written = 0;
                unsafe {
                    crate::properties::ffi::mr_hal_core_property_get(
                        registry,
                        id,
                        &raw const address,
                        0,
                        std::ptr::null(),
                        64,
                        &raw mut written,
                        value.as_mut_ptr().cast(),
                        0,
                    )
                }
            };
            let name = read(*b"lnam", u32::from_be_bytes(*b"glob"));
            let latency = read(*b"ltnc", u32::from_be_bytes(*b"inpt"));
            done.send((name, latency)).unwrap();
        });
        let result = finished.recv_timeout(Duration::from_millis(200));
        drop(control);
        result
    });
    assert_eq!(timely.unwrap(), (i32::from_be_bytes(*b"nope"), OK));
}

/// Every selector the property catalog answers on any object.
const CATALOG_SELECTORS: [&[u8; 4]; 48] = [
    b"bcls", b"clas", b"stdv", b"lnam", b"lmak", b"rsrc", b"ownd", b"dev#", b"box#", b"clk#",
    b"uidd", b"uid ", b"muid", b"akin", b"clkd", b"hidn", b"livn", b"goin", b"dflt", b"sflt",
    b"ltnc", b"saft", b"nsrt", b"nsr#", b"ring", b"clok", b"cstb", b"dch2", b"srnd", b"stm#",
    b"ctrl", b"tran", b"sact", b"sdir", b"term", b"schn", b"sfmt", b"pft ", b"sfma", b"pfta",
    b"cscp", b"celm", b"lcsv", b"lcdv", b"lcdr", b"lcsd", b"lcds", b"bcvl",
];

/// Has/size/get of one property, asserting none of them allocates. `Some` holds the
/// 4-byte answer; `None` means the read was refused.
fn read_without_allocation(
    registry: *mut crate::registry::Registry,
    object: u32,
    address: &Address,
    requester: i32,
) -> Option<u32> {
    use crate::properties::ffi::{mr_hal_core_has_property, mr_hal_core_property_size};
    let mut output = [0_u8; 64];
    let (mut size, mut written, mut status) = (0, 0, OK);
    let measured = allocation_counter::measure(|| {
        // SAFETY: the caller's fixture keeps the registry alive; the address and outputs
        // are live for the call and output holds the 64 bytes offered.
        unsafe {
            mr_hal_core_has_property(registry, object, address);
            mr_hal_core_property_size(registry, object, address, &raw mut size);
            status = mr_hal_core_property_get(
                registry,
                object,
                address,
                0,
                std::ptr::null(),
                64,
                &raw mut written,
                output.as_mut_ptr().cast(),
                requester,
            );
        }
    });
    assert_eq!(
        measured.count_total,
        0,
        "{} {} on object {object} for PID {requester}",
        String::from_utf8_lossy(&address.selector.to_be_bytes()),
        String::from_utf8_lossy(&address.scope.to_be_bytes()),
    );
    (status == OK).then(|| {
        assert_eq!(written, 4);
        u32::from_ne_bytes(output[..4].try_into().unwrap())
    })
}

/// IO and timestamp callbacks may reenter property reads. Every read there, answered or
/// refused, stays off the allocator; only inline answers that need no control lock remain.
#[test]
fn audio_thread_property_reads_never_allocate() {
    use mixrelay_macos_transport::control_inventory::InputLatency;
    const APP: i32 = 4242;
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    fixture.registry.publish_latencies(&[InputLatency {
        endpoint_id: [1; 16],
        rate: 48_000,
        engine_pid: 10,
        lease: 1,
        revision: 1,
        frames: Some(321),
    }]);
    let registry = (&raw const *fixture.registry).cast_mut();
    let mut answered = Vec::new();
    let audio = crate::transport::AudioScope::enter();
    for object in [1, id, id + 1, id + 2, id + 3, id + 4] {
        for selector in CATALOG_SELECTORS {
            for scope in [*b"glob", *b"inpt", *b"outp"] {
                let address = Address {
                    selector: u32::from_be_bytes(*selector),
                    scope: u32::from_be_bytes(scope),
                    element: 0,
                };
                for requester in [0, APP] {
                    if let Some(value) =
                        read_without_allocation(registry, object, &address, requester)
                    {
                        answered.push((object, *selector, scope, requester, value));
                    }
                }
            }
        }
    }
    drop(audio);
    let mut expected = vec![
        // The host's zero scheduling latency is independent of app measurements.
        (id, *b"ltnc", *b"inpt", APP, 321),
        (id, *b"ltnc", *b"inpt", 0, 0),
    ];
    for requester in [0, APP] {
        expected.extend([
            (
                1,
                *b"bcls",
                *b"glob",
                requester,
                u32::from_be_bytes(*b"aobj"),
            ),
            (
                1,
                *b"clas",
                *b"glob",
                requester,
                u32::from_be_bytes(*b"aplg"),
            ),
            (1, *b"stdv", *b"glob", requester, 0),
            (id, *b"ltnc", *b"outp", requester, 0),
            (id, *b"saft", *b"inpt", requester, 0),
            (id, *b"saft", *b"outp", requester, 0),
            (id + 1, *b"ltnc", *b"glob", requester, 0),
            (id + 2, *b"ltnc", *b"glob", requester, 0),
        ]);
        for scope in [*b"glob", *b"inpt", *b"outp"] {
            expected.push((id, *b"tran", scope, requester, u32::from_be_bytes(*b"virt")));
            expected.extend([
                (id, *b"ring", scope, requester, 16_384),
                (id, *b"clok", scope, requester, u32::from_be_bytes(*b"raww")),
                (id, *b"cstb", scope, requester, 1),
            ]);
        }
    }
    answered.sort_unstable();
    expected.sort_unstable();
    assert_eq!(answered, expected);
}

/// A UID lookup reads its `CFString` qualifier into an owned string. On an IO thread the
/// lookup is refused before that parse, so even a valid qualifier allocates nothing.
#[test]
fn audio_thread_uid_lookup_is_refused_before_parsing_its_qualifier() {
    use std::ffi::c_void;
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let registry = (&raw const *fixture.registry).cast_mut();
    let device_uid = Address {
        selector: u32::from_be_bytes(*b"uid "),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let uid = usize::from_ne_bytes(fixture.property::<8>(id, &device_uid)) as *const c_void;
    assert!(!uid.is_null());
    let lookup = Address {
        selector: u32::from_be_bytes(*b"uidd"),
        ..device_uid
    };
    let read = |found: &mut u32| {
        let mut written = 0;
        // SAFETY: the fixture keeps the registry alive; the qualifier holds one live
        // `CFStringRef` and `found` is a writable `u32` for the synchronous call.
        unsafe {
            mr_hal_core_property_get(
                registry,
                1,
                &raw const lookup,
                u32::try_from(size_of::<*const c_void>()).unwrap(),
                (&raw const uid).cast(),
                4,
                &raw mut written,
                (&raw mut *found).cast(),
                0,
            )
        }
    };
    // The qualifier is valid: off the IO thread it resolves the device.
    let mut found = 0;
    assert_eq!(read(&mut found), OK);
    assert_eq!(found, id);

    let mut status = OK;
    let audio = crate::transport::AudioScope::enter();
    let measured = allocation_counter::measure(|| status = read(&mut found));
    drop(audio);
    // SAFETY: `uid` is the +1 reference the `uid ` read returned; nothing uses it afterwards.
    unsafe { CFRelease(uid) };
    assert_eq!(measured.count_total, 0);
    assert_eq!(status, i32::from_be_bytes(*b"nope"));
}
