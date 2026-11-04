//! Codec contract tests. Helpers build valid snapshots over anonymous shared memory.
use super::policy::*;
use super::snapshot::*;
use super::xpc::*;
use super::*;
use std::{
    ffi::{CString, c_char},
    fs::File,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    ptr,
};

mod endpoint_requests;
mod policy;
mod snapshot;

pub(super) fn backing() -> OwnedFd {
    let descriptor = crate::anonymous_memory::mr_shared_memory_create(4096, true);
    assert!(descriptor >= 0);
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let metadata = File::from(owned.try_clone().unwrap()).metadata().unwrap();
    assert_eq!(metadata.mode() & 0o170_000, 0, "Darwin POSIX shm type bits");
    assert!(metadata.len() >= 4096, "Darwin POSIX shm backing length");
    owned
}

pub(super) fn entry(descriptor: &OwnedFd, index: u8, name: &str) -> InventoryEntry {
    let mut entry = InventoryEntry {
        endpoint_id: [index; 16],
        sample_rate: 48_000,
        descriptor: descriptor.as_raw_fd(),
        mapped_size: 4096,
        generation: u64::from(index),
        ..InventoryEntry::default()
    };
    for (target, byte) in entry.display_name.iter_mut().zip(name.bytes()) {
        *target = c_char::from_ne_bytes([byte]);
    }
    entry
}

pub(super) fn message(entries: &[InventoryEntry]) -> Message {
    Message::new(unsafe {
        mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), 7)
    })
    .expect("valid inventory")
}

pub(super) struct Snapshot(pub(super) InventorySnapshot);
impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe { mr_hal_control_inventory_snapshot_close(&raw mut self.0) };
    }
}

pub(super) fn decode(message: &Message) -> Option<Snapshot> {
    let mut result = Snapshot(InventorySnapshot::default());
    unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut result.0) }
        .then_some(result)
}

pub(super) fn item(message: &Message, index: usize) -> Xpc {
    unsafe { xpc_array_get_value(field(message.0, c"entries"), index) }
}

pub(super) fn unchanged_on_error(message: &Message) {
    let mut output = InventorySnapshot {
        generation: 99,
        ..InventorySnapshot::default()
    };
    assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
    assert_eq!(output.generation, 99);
    assert_eq!(output.count, 0);
    assert!(output.entries.iter().all(|entry| entry.descriptor == -1));
}
