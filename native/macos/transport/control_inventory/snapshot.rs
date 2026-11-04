//! Authoritative inventory snapshots and their requests. Encoding borrows each entry's
//! descriptor; decoding owns duplicates and publishes nothing unless every entry is valid.
use super::policy::{ProducerPolicy, TransportCompatibility, parse_policy, write_policy};
use super::write_input_latencies;
use super::xpc::{
    _xpc_type_array, _xpc_type_fd, Message, Xpc, dictionary, endpoint_id, field, is_type,
    new_message, operation, string, uint, xpc_array_append_value, xpc_array_create,
    xpc_array_get_count, xpc_array_get_value, xpc_dictionary_create, xpc_dictionary_create_reply,
    xpc_dictionary_dup_fd, xpc_dictionary_set_data, xpc_dictionary_set_fd,
    xpc_dictionary_set_string, xpc_dictionary_set_uint64, xpc_dictionary_set_value,
};
use super::{MAX_ENTRIES, NAME_BYTES};
use std::{
    ffi::{CStr, c_char, c_int},
    fs::File,
    os::fd::{BorrowedFd, FromRawFd, IntoRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    ptr,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct InventoryEntry {
    pub endpoint_id: [u8; 16],
    pub display_name: [c_char; NAME_BYTES],
    pub sample_rate: u32,
    pub descriptor: c_int,
    pub mapped_size: usize,
    pub generation: u64,
}

impl Default for InventoryEntry {
    fn default() -> Self {
        Self {
            endpoint_id: [0; 16],
            display_name: [0; NAME_BYTES],
            sample_rate: 0,
            descriptor: -1,
            mapped_size: 0,
            generation: 0,
        }
    }
}

#[repr(C)]
pub struct InventorySnapshot {
    pub generation: u64,
    pub count: usize,
    pub entries: [InventoryEntry; MAX_ENTRIES],
    pub transport_compatibility: TransportCompatibility,
}

impl Default for InventorySnapshot {
    fn default() -> Self {
        Self {
            generation: 0,
            count: 0,
            entries: [InventoryEntry::default(); MAX_ENTRIES],
            transport_compatibility: TransportCompatibility::default(),
        }
    }
}

pub(super) fn entry_name(entry: &InventoryEntry) -> Option<&CStr> {
    let bytes =
        unsafe { std::slice::from_raw_parts(entry.display_name.as_ptr().cast::<u8>(), NAME_BYTES) };
    let value = CStr::from_bytes_until_nul(bytes).ok()?;
    (!value.is_empty() && value.to_str().is_ok()).then_some(value)
}
pub(super) fn valid_backing(file: &File, size: usize) -> bool {
    size > 0
        && u32::try_from(size).is_ok()
        && file.metadata().is_ok_and(|metadata| {
            // Darwin pshm_stat reports only shm_open permission bits, not
            // S_IFREG (XNU bsd/kern/posix_shm.c). The transport allocator
            // exclusively creates anonymous POSIX shared-memory objects.
            metadata.mode() & 0o170_000 == 0 && metadata.len() >= size as u64
        })
}
fn input_valid(entries: &[InventoryEntry], generation: u64) -> bool {
    generation != 0
        && entries.len() <= MAX_ENTRIES
        && entries.iter().enumerate().all(|(index, entry)| {
            if entry.generation == 0
                || entry.sample_rate == 0
                || entry.descriptor < 0
                || entry_name(entry).is_none()
                || entry.endpoint_id == [0; 16]
                || entries[..index]
                    .iter()
                    .any(|old| old.endpoint_id == entry.endpoint_id)
            {
                return false;
            }
            // Borrow the producer's FD. The temporary clone and XPC's later copy are
            // independently owned; no creator error may close caller storage.
            unsafe { BorrowedFd::borrow_raw(entry.descriptor) }
                .try_clone_to_owned()
                .map(File::from)
                .is_ok_and(|file| valid_backing(&file, entry.mapped_size))
        })
}
fn populate(
    message: Message,
    entries: &[InventoryEntry],
    generation: u64,
    policy: &ProducerPolicy,
) -> Option<Message> {
    let array = Message::new(unsafe { xpc_array_create(ptr::null(), 0) })?;
    for entry in entries {
        let item = Message::new(unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) })?;
        let name = entry_name(entry)?;
        unsafe {
            xpc_dictionary_set_data(
                item.0,
                c"endpoint_id".as_ptr(),
                entry.endpoint_id.as_ptr().cast(),
                16,
            );
            xpc_dictionary_set_string(item.0, c"display_name".as_ptr(), name.as_ptr());
            xpc_dictionary_set_uint64(
                item.0,
                c"sample_rate".as_ptr(),
                u64::from(entry.sample_rate),
            );
            xpc_dictionary_set_uint64(item.0, c"mapped_size".as_ptr(), entry.mapped_size as u64);
            xpc_dictionary_set_uint64(item.0, c"generation".as_ptr(), entry.generation);
            xpc_dictionary_set_fd(item.0, c"transport_fd".as_ptr(), entry.descriptor);
            xpc_array_append_value(array.0, item.0);
        }
    }
    unsafe {
        xpc_dictionary_set_uint64(message.0, c"version".as_ptr(), 1);
        xpc_dictionary_set_string(
            message.0,
            c"operation".as_ptr(),
            c"inventory_snapshot".as_ptr(),
        );
        xpc_dictionary_set_uint64(message.0, c"generation".as_ptr(), generation);
        xpc_dictionary_set_value(message.0, c"entries".as_ptr(), array.0);
    }
    write_policy(message.0, &policy.0)?;
    // Every snapshot carries the latency list; the service replaces it with current entries.
    // SAFETY: `message` is the live dictionary this function exclusively owns.
    unsafe { write_input_latencies(message.0, &[]) }.then_some(message)
}
fn parse_entry(item: Xpc) -> Option<(InventoryEntry, OwnedFd)> {
    if !dictionary(item) {
        return None;
    }
    let id = endpoint_id(item)?;
    let name = string(field(item, c"display_name"))?;
    let sample_rate = u32::try_from(uint(item, c"sample_rate")?)
        .ok()
        .filter(|rate| *rate != 0)?;
    let mapped_size = usize::try_from(uint(item, c"mapped_size")?).ok()?;
    let generation = uint(item, c"generation").filter(|generation| *generation != 0)?;
    if name.is_empty()
        || !is_type(
            field(item, c"transport_fd"),
            (&raw const _xpc_type_fd).cast(),
        )
    {
        return None;
    }
    let descriptor = unsafe { xpc_dictionary_dup_fd(item, c"transport_fd".as_ptr()) };
    if descriptor < 0 {
        return None;
    }
    let file = File::from(unsafe { OwnedFd::from_raw_fd(descriptor) });
    if !valid_backing(&file, mapped_size) {
        return None;
    }
    let mut entry = InventoryEntry {
        endpoint_id: id,
        sample_rate,
        mapped_size,
        generation,
        ..InventoryEntry::default()
    };
    for (target, byte) in entry.display_name.iter_mut().zip(name.to_bytes_with_nul()) {
        *target = c_char::from_ne_bytes([*byte]);
    }
    Some((entry, file.into()))
}

/// Creates a v1 inventory request; caller owns the returned XPC object.
#[unsafe(no_mangle)]
pub extern "C" fn mr_hal_control_create_inventory_request() -> Xpc {
    new_message(c"request_inventory").map_or(ptr::null_mut(), Message::into_raw)
}

/// # Safety
/// Non-null input must be a live XPC object for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_inventory_request(message: Xpc) -> bool {
    operation(message, c"request_inventory")
}

/// Encodes a snapshot with this service owner's explicit presentation policy.
/// # Safety
/// Every entry's descriptor is borrowed, live storage retained throughout encoding.
pub unsafe fn create_inventory_snapshot(
    entries: &[InventoryEntry],
    generation: u64,
    policy: &ProducerPolicy,
) -> Xpc {
    if !input_valid(entries, generation) {
        return ptr::null_mut();
    }
    new_message(c"inventory_snapshot")
        .and_then(|message| populate(message, entries, generation, policy))
        .map_or(ptr::null_mut(), Message::into_raw)
}

/// # Safety
/// Entries must reference count readable ABI records with borrowed live FDs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_inventory_snapshot(
    entries: *const InventoryEntry,
    count: usize,
    generation: u64,
) -> Xpc {
    if count > MAX_ENTRIES || (count != 0 && entries.is_null()) {
        return ptr::null_mut();
    }
    let entries = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(entries, count) }
    };
    // SAFETY: the ABI caller retains every borrowed descriptor for this operation.
    unsafe { create_inventory_snapshot(entries, generation, &ProducerPolicy::default()) }
}

/// # Safety
/// Request must be a live XPC object; entries follow the creator contract above.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_inventory_snapshot_reply(
    request: Xpc,
    entries: *const InventoryEntry,
    count: usize,
    generation: u64,
) -> Xpc {
    if !operation(request, c"request_inventory")
        || count > MAX_ENTRIES
        || (count != 0 && entries.is_null())
    {
        return ptr::null_mut();
    }
    let entries = if count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(entries, count) }
    };
    if !input_valid(entries, generation) {
        return ptr::null_mut();
    }
    Message::new(unsafe { xpc_dictionary_create_reply(request) })
        .and_then(|message| populate(message, entries, generation, &ProducerPolicy::default()))
        .map_or(ptr::null_mut(), Message::into_raw)
}

/// # Safety
/// Message must be live XPC; output must be writable, empty/previously closed,
/// and not alias message storage. Output is untouched on every validation error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_inventory_snapshot(
    message: Xpc,
    output: *mut InventorySnapshot,
) -> bool {
    if output.is_null() || !operation(message, c"inventory_snapshot") {
        return false;
    }
    let Some(generation) = uint(message, c"generation").filter(|generation| *generation != 0)
    else {
        return false;
    };
    let array = field(message, c"entries");
    if !is_type(array, (&raw const _xpc_type_array).cast()) {
        return false;
    }
    let count = unsafe { xpc_array_get_count(array) };
    if count > MAX_ENTRIES {
        return false;
    }
    let Some(transport_compatibility) = parse_policy(message) else {
        return false;
    };
    let mut decoded = Vec::with_capacity(count);
    for index in 0..count {
        let Some((entry, descriptor)) = parse_entry(unsafe { xpc_array_get_value(array, index) })
        else {
            return false;
        };
        if decoded
            .iter()
            .any(|(old, _): &(InventoryEntry, OwnedFd)| old.endpoint_id == entry.endpoint_id)
        {
            return false;
        }
        decoded.push((entry, descriptor));
    }
    let mut complete = InventorySnapshot {
        generation,
        count,
        transport_compatibility,
        ..InventorySnapshot::default()
    };
    for (index, (mut entry, descriptor)) in decoded.into_iter().enumerate() {
        entry.descriptor = descriptor.into_raw_fd();
        complete.entries[index] = entry;
    }
    unsafe { output.write(complete) };
    true
}

/// # Safety
/// A non-null pointer must contain a successful snapshot or a zero-count empty
/// snapshot, with unique owned FDs. Closing twice is safe after the first reset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_inventory_snapshot_close(snapshot: *mut InventorySnapshot) {
    let Some(snapshot) = (unsafe { snapshot.as_mut() }) else {
        return;
    };
    for entry in snapshot
        .entries
        .iter_mut()
        .take(snapshot.count.min(MAX_ENTRIES))
    {
        if entry.descriptor >= 0 {
            drop(unsafe { OwnedFd::from_raw_fd(entry.descriptor) });
        }
    }
    *snapshot = InventorySnapshot::default();
}
