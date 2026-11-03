use std::ffi::{c_char, c_void};
use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;

use mixrelay_macos_transport::control_inventory::{
    InventoryEntry, mr_hal_control_create_inventory_snapshot,
};

use super::{FAIL_CREATE_ENDPOINT, Fixture, LIVE, native_tests::Transport};

unsafe extern "C" {
    fn xpc_release(message: *mut c_void);
}

struct Message(*mut c_void);
impl Message {
    fn new(endpoints: &[u8], generation: u64) -> Self {
        Self::with_entry_generation(endpoints, generation, generation)
    }

    fn with_entry_generation(endpoints: &[u8], generation: u64, entry_generation: u64) -> Self {
        let transports: Vec<_> = endpoints
            .iter()
            .map(|endpoint| Transport::new(*endpoint, 2, 2))
            .collect();
        let entries: Vec<_> = endpoints
            .iter()
            .zip(&transports)
            .map(|(endpoint, transport)| {
                let mut entry = InventoryEntry {
                    endpoint_id: [*endpoint; 16],
                    sample_rate: 48_000,
                    descriptor: transport.fd.as_raw_fd(),
                    mapped_size: transport.size,
                    generation: entry_generation,
                    ..InventoryEntry::default()
                };
                for (slot, byte) in entry
                    .display_name
                    .iter_mut()
                    .zip(format!("Device {endpoint}").bytes())
                {
                    *slot = c_char::from_ne_bytes([byte]);
                }
                entry
            })
            .collect();
        let message = unsafe {
            mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), generation)
        };
        assert!(!message.is_null());
        Self(message)
    }

    fn apply(&self, fixture: &Fixture) -> bool {
        unsafe {
            assert!(crate::inventory::mr_hal_core_control_connected(
                (&raw const *fixture.registry).cast_mut(),
                1,
            ));
            crate::inventory::mr_hal_core_control_message(
                (&raw const *fixture.registry).cast_mut(),
                1,
                self.0,
            )
        }
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0) };
    }
}

fn descriptor_count() -> usize {
    std::fs::read_dir("/dev/fd").unwrap().count()
}

fn inventory_fixture() -> Fixture {
    unsafe extern "C" fn inspect(fd: i32, size: usize, output: *mut crate::abi::Geometry) -> bool {
        let Ok(config) =
            mixrelay_macos_transport::transport_inspection::inspect_descriptor(fd, size)
        else {
            return false;
        };
        // SAFETY: the registry supplies one writable aligned geometry output;
        // descriptor inspection borrows the live anonymous transport backing.
        unsafe {
            output.write(crate::abi::Geometry {
                endpoint_id: config.endpoint_id,
                render_channels: config.render_channels,
                capture_channels: config.capture_channels,
            });
        };
        true
    }
    let mut fixture = Fixture::new();
    fixture.registry.native.inspect = inspect;
    fixture
}

#[test]
fn inventory_ack_waits_for_perform_and_newer_publication_can_restore_older_mapping() {
    let fixture = inventory_fixture();
    let initial = Message::new(&[1], 90);
    assert!(initial.apply(&fixture));
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((90, crate::inventory::Settlement::Applied))
    );
    assert_eq!(crate::inventory::settled(&fixture.registry, 2), None);
    let candidate = Message::new(&[1], 91);
    assert!(candidate.apply(&fixture));
    assert_eq!(crate::inventory::settled(&fixture.registry, 1), None);
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((91, crate::inventory::Settlement::Applied))
    );
    let rollback = Message::with_entry_generation(&[1], 92, 90);
    assert!(rollback.apply(&fixture));
    assert_eq!(crate::inventory::settled(&fixture.registry, 1), None);
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((92, crate::inventory::Settlement::Applied))
    );
}

#[test]
fn failed_inventory_apply_settles_as_rejected_until_a_newer_snapshot_applies() {
    let fixture = inventory_fixture();
    assert!(Message::new(&[1], 90).apply(&fixture));
    // A native create failure used to leave the generation unsettled, so the Driver
    // service waited out its whole budget before reporting the change pending.
    FAIL_CREATE_ENDPOINT.store(2, Ordering::SeqCst);
    assert!(!Message::new(&[1, 2], 91).apply(&fixture));
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((91, crate::inventory::Settlement::Rejected))
    );
    FAIL_CREATE_ENDPOINT.store(0, Ordering::SeqCst);
    assert!(Message::with_entry_generation(&[1], 92, 90).apply(&fixture));
    // The failed snapshot already queued endpoint 1's newer mapping; restoring needs a Perform.
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((92, crate::inventory::Settlement::Applied))
    );
}

#[test]
fn aborted_attachment_settles_as_rejected_until_a_newer_publication_applies() {
    let fixture = inventory_fixture();
    assert!(Message::new(&[1], 90).apply(&fixture));
    assert!(Message::new(&[1], 91).apply(&fixture));
    let (id, generation) = fixture.pending();
    assert_eq!(crate::inventory::settled(&fixture.registry, 1), None);
    // The Driver service learns of the abort instead of waiting for an ACK that never comes.
    assert_eq!(
        fixture
            .registry
            .abort(id, crate::abi::CONFIG_ACTION, generation),
        crate::abi::OK
    );
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((91, crate::inventory::Settlement::Rejected))
    );
    assert!(Message::with_entry_generation(&[1], 92, 90).apply(&fixture));
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((92, crate::inventory::Settlement::Applied))
    );
}

#[test]
fn a_failed_rate_request_keeps_an_earlier_mapping_failure_rejected_and_retried() {
    let fixture = inventory_fixture();
    assert!(Message::new(&[1], 90).apply(&fixture));
    assert!(Message::new(&[1], 91).apply(&fixture));
    let (id, generation) = fixture.pending();
    assert_eq!(
        fixture
            .registry
            .abort(id, crate::abi::CONFIG_ACTION, generation),
        crate::abi::OK
    );
    // A later rate-only request that also fails must not clear the mapping failure:
    // it used to report generation 91 as Applied and stop every retry.
    assert_eq!(fixture.registry.set_rate(id, 96_000), crate::abi::OK);
    let (_, rate_generation) = fixture.pending();
    assert_eq!(
        fixture
            .registry
            .abort(id, crate::abi::CONFIG_ACTION, rate_generation),
        crate::abi::OK
    );
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((91, crate::inventory::Settlement::Rejected))
    );
    // The same publication maps the endpoint again.
    assert!(Message::new(&[1], 91).apply(&fixture));
    let (_, retry) = fixture.pending();
    assert!(retry > rate_generation);
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((91, crate::inventory::Settlement::Applied))
    );
}

#[test]
fn replacement_requests_require_current_validated_inventory() {
    unsafe extern "C" fn requires_replacement(_: *mut c_void) -> bool {
        true
    }
    let mut fixture = inventory_fixture();
    fixture.registry.native.requires_replacement = requires_replacement;
    let inventory = Message::new(&[1], 90);
    assert!(inventory.apply(&fixture));
    let request = |authority| {
        let mut endpoint = [0; 16];
        let mut generation = 0;
        let accepted = unsafe {
            crate::inventory::mr_hal_core_replacement_request(
                (&raw const *fixture.registry).cast_mut(),
                authority,
                &raw mut endpoint,
                &raw mut generation,
            )
        };
        (accepted, endpoint, generation)
    };
    assert_eq!(request(1), (true, [1; 16], 90));
    assert!(unsafe {
        crate::inventory::mr_hal_core_control_connected(
            (&raw const *fixture.registry).cast_mut(),
            2,
        )
    });
    assert_eq!(request(1), (false, [0; 16], 0));
    assert_eq!(request(2), (false, [0; 16], 0));
    let new = Message::new(&[1], 2);
    assert!(unsafe {
        crate::inventory::mr_hal_core_control_message(
            (&raw const *fixture.registry).cast_mut(),
            2,
            new.0,
        )
    });
    assert_eq!(fixture.perform(), crate::abi::OK);
    assert_eq!(request(2), (true, [1; 16], 2));
}

#[test]
fn partial_publication_is_removed_by_next_empty_inventory_without_leaks() {
    let fixture = inventory_fixture();
    let before = descriptor_count();
    FAIL_CREATE_ENDPOINT.store(2, Ordering::SeqCst);
    let partial = Message::new(&[1, 2], 2);
    assert!(!partial.apply(&fixture));
    assert_eq!(fixture.registry.devices().len(), 1);
    assert_eq!(LIVE.load(Ordering::SeqCst), 1);
    // An older complete inventory must not erase partially applied newer intent.
    let stale = Message::new(&[], 1);
    assert!(stale.apply(&fixture));
    assert_eq!(fixture.registry.devices().len(), 1);
    let empty = Message::new(&[], 3);
    assert!(empty.apply(&fixture));
    assert!(fixture.registry.devices().is_empty());
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
    drop((partial, stale, empty));
    assert_eq!(descriptor_count(), before);
}

#[test]
fn same_generation_retry_completes_after_native_allocation_recovers() {
    let fixture = inventory_fixture();
    FAIL_CREATE_ENDPOINT.store(2, Ordering::SeqCst);
    let inventory = Message::new(&[1, 2], 1);
    assert!(!inventory.apply(&fixture));
    let first = fixture.registry.devices()[0];
    FAIL_CREATE_ENDPOINT.store(0, Ordering::SeqCst);
    assert!(inventory.apply(&fixture));
    assert_eq!(fixture.registry.devices()[0], first);
    assert_eq!(LIVE.load(Ordering::SeqCst), 2);
}

#[test]
fn full_registry_can_replace_one_absent_idle_device() {
    let fixture = inventory_fixture();
    let initial = Message::new(&(1..=64).collect::<Vec<_>>(), 1);
    assert!(initial.apply(&fixture));
    let original = fixture.registry.devices();
    assert_eq!(original.len(), 64);
    let replacement = Message::new(&(2..=65).collect::<Vec<_>>(), 2);
    assert!(replacement.apply(&fixture));
    let current = fixture.registry.devices();
    assert_eq!(current.len(), 64);
    assert!(!current.contains(&original[0]));
    assert_eq!(LIVE.load(Ordering::SeqCst), 64);
    let empty = Message::new(&[], 3);
    assert!(empty.apply(&fixture));
    assert_eq!(LIVE.load(Ordering::SeqCst), 0);
}

#[test]
fn transport_policy_obeys_authority_generation_and_last_valid_snapshot() {
    unsafe extern "C" {
        fn xpc_dictionary_set_value(message: *mut c_void, key: *const c_char, value: *mut c_void);
        fn xpc_dictionary_set_uint64(message: *mut c_void, key: *const c_char, value: u64);
        fn xpc_array_create(values: *const *mut c_void, count: usize) -> *mut c_void;
        fn xpc_array_append_value(array: *mut c_void, value: *mut c_void);
        fn xpc_string_create(value: *const c_char) -> *mut c_void;
    }
    let fixture = Fixture::new();
    let pid = i32::try_from(std::process::id()).unwrap();
    fixture
        .registry
        .transport
        .client(16, 1, pid, Some("com.test.App".into()));
    let message = Message::new(&[], 10);
    let array = unsafe { xpc_array_create(std::ptr::null(), 0) };
    let bundle = unsafe { xpc_string_create(c"com.test.App".as_ptr()) };
    unsafe {
        xpc_array_append_value(array, bundle);
        xpc_dictionary_set_value(message.0, c"transport_compatibility".as_ptr(), array);
        xpc_release(bundle);
        xpc_release(array);
    }
    assert!(message.apply(&fixture));
    assert_eq!(
        fixture.registry.transport.get(16, pid),
        u32::from_be_bytes(*b"usb ")
    );
    let stale = Message::new(&[], 9);
    assert!(stale.apply(&fixture));
    assert_eq!(
        fixture.registry.transport.get(16, pid),
        u32::from_be_bytes(*b"usb ")
    );
    unsafe {
        xpc_dictionary_set_uint64(message.0, c"generation".as_ptr(), 11);
    }
    unsafe {
        xpc_dictionary_set_uint64(message.0, c"transport_compatibility".as_ptr(), 7);
    }
    assert!(!message.apply(&fixture));
    assert_eq!(
        crate::inventory::settled(&fixture.registry, 1),
        Some((10, crate::inventory::Settlement::Applied))
    );
    assert_eq!(
        fixture.registry.transport.get(16, pid),
        u32::from_be_bytes(*b"usb ")
    );
    let cleared = Message::new(&[], 12);
    assert!(cleared.apply(&fixture));
    assert_eq!(
        fixture.registry.transport.get(16, pid),
        u32::from_be_bytes(*b"virt")
    );
}
