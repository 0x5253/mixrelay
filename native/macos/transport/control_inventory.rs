//! Versioned XPC inventory codec. This module owns message/descriptor lifetimes,
//! not the Driver service or HAL registry. A failed snapshot never publishes a partial
//! authoritative list, so malformed traffic cannot trigger device removal.
#![cfg(target_os = "macos")]

mod latency;
pub mod report;
pub use latency::{InputLatency, parse_input_latencies, write_input_latencies};

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    fs::File,
    os::fd::{BorrowedFd, FromRawFd, IntoRawFd, OwnedFd},
    os::unix::fs::MetadataExt,
    ptr,
};

pub const MAX_ENTRIES: usize = 64;
pub const NAME_BYTES: usize = 128;
pub const MAX_TRANSPORT_BUNDLES: usize = 32;
/// Immutable transport presentation policy owned by one publishing service.
#[derive(Debug, Clone, Default)]
pub struct ProducerPolicy(Vec<CString>);

impl ProducerPolicy {
    /// Rejects malformed, duplicate or excessive bundle identifiers before publication.
    #[must_use]
    pub fn new(bundles: &[String]) -> Option<Self> {
        if bundles.len() > MAX_TRANSPORT_BUNDLES
            || bundles
                .iter()
                .enumerate()
                .any(|(index, bundle)| !valid_bundle(bundle) || bundles[..index].contains(bundle))
        {
            return None;
        }
        bundles
            .iter()
            .map(|bundle| CString::new(bundle.as_str()))
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .map(Self)
    }
}

fn valid_bundle(bundle: &str) -> bool {
    !bundle.is_empty()
        && bundle.len() < NAME_BYTES
        && bundle.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TransportCompatibility {
    pub count: usize,
    pub bundles: [[c_char; NAME_BYTES]; MAX_TRANSPORT_BUNDLES],
}
impl Default for TransportCompatibility {
    fn default() -> Self {
        Self {
            count: 0,
            bundles: [[0; NAME_BYTES]; MAX_TRANSPORT_BUNDLES],
        }
    }
}
impl TransportCompatibility {
    /// Returns the bounded, terminated identifiers copied by the decoder.
    #[must_use]
    pub fn identifiers(&self) -> Vec<String> {
        self.bundles
            .iter()
            .take(self.count)
            .filter_map(|bytes| {
                let bytes =
                    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), NAME_BYTES) };
                CStr::from_bytes_until_nul(bytes)
                    .ok()?
                    .to_str()
                    .ok()
                    .map(str::to_owned)
            })
            .collect()
    }
}

pub(super) type Xpc = *mut c_void;
type XpcType = *const c_void;

unsafe extern "C" {
    static _xpc_type_dictionary: c_char;
    static _xpc_type_array: c_char;
    static _xpc_type_uint64: c_char;
    static _xpc_type_string: c_char;
    static _xpc_type_data: c_char;
    static _xpc_type_fd: c_char;
    fn xpc_get_type(value: Xpc) -> XpcType;
    fn xpc_release(value: Xpc);
    fn xpc_dictionary_create(keys: *const *const c_char, values: *const Xpc, count: usize) -> Xpc;
    pub(super) fn xpc_dictionary_create_reply(request: Xpc) -> Xpc;
    fn xpc_dictionary_get_value(dictionary: Xpc, key: *const c_char) -> Xpc;
    fn xpc_dictionary_set_value(dictionary: Xpc, key: *const c_char, value: Xpc);
    pub(super) fn xpc_dictionary_set_uint64(dictionary: Xpc, key: *const c_char, value: u64);
    pub(super) fn xpc_dictionary_set_string(
        dictionary: Xpc,
        key: *const c_char,
        value: *const c_char,
    );
    pub(super) fn xpc_dictionary_set_data(
        dictionary: Xpc,
        key: *const c_char,
        value: *const c_void,
        length: usize,
    );
    pub(super) fn xpc_dictionary_set_fd(dictionary: Xpc, key: *const c_char, descriptor: c_int);
    pub(super) fn xpc_dictionary_dup_fd(dictionary: Xpc, key: *const c_char) -> c_int;
    fn xpc_uint64_get_value(value: Xpc) -> u64;
    fn xpc_string_get_string_ptr(value: Xpc) -> *const c_char;
    fn xpc_string_get_length(value: Xpc) -> usize;
    fn xpc_data_get_bytes_ptr(value: Xpc) -> *const c_void;
    fn xpc_data_get_length(value: Xpc) -> usize;
    fn xpc_string_create(value: *const c_char) -> Xpc;
    fn xpc_array_create(values: *const Xpc, count: usize) -> Xpc;
    fn xpc_array_append_value(array: Xpc, value: Xpc);
    fn xpc_array_get_count(array: Xpc) -> usize;
    fn xpc_array_get_value(array: Xpc, index: usize) -> Xpc;
}

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

pub(super) struct Message(pub(super) Xpc);
impl Message {
    pub(super) fn new(raw: Xpc) -> Option<Self> {
        if raw.is_null() { None } else { Some(Self(raw)) }
    }
    pub(super) fn into_raw(self) -> Xpc {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        unsafe { xpc_release(self.0) };
    }
}

fn is_type(value: Xpc, expected: XpcType) -> bool {
    !value.is_null() && unsafe { xpc_get_type(value) } == expected
}
pub(super) fn dictionary(value: Xpc) -> bool {
    is_type(value, (&raw const _xpc_type_dictionary).cast())
}
fn field(message: Xpc, key: &CStr) -> Xpc {
    unsafe { xpc_dictionary_get_value(message, key.as_ptr()) }
}
pub(super) fn uint(message: Xpc, key: &CStr) -> Option<u64> {
    let value = field(message, key);
    is_type(value, (&raw const _xpc_type_uint64).cast())
        .then(|| unsafe { xpc_uint64_get_value(value) })
}
fn string(value: Xpc) -> Option<CString> {
    if !is_type(value, (&raw const _xpc_type_string).cast()) {
        return None;
    }
    let length = unsafe { xpc_string_get_length(value) };
    if length >= NAME_BYTES {
        return None;
    }
    let pointer = unsafe { xpc_string_get_string_ptr(value) };
    if pointer.is_null() {
        return None;
    }
    // The XPC string owns length+1 bytes. Copy the bounded view so no borrow can
    // outlive its parent message, including during an early validation return.
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length + 1) };
    let value = CStr::from_bytes_with_nul(bytes).ok()?;
    value.to_str().ok()?;
    Some(value.to_owned())
}
pub(super) fn operation(message: Xpc, expected: &CStr) -> bool {
    dictionary(message)
        && uint(message, c"version") == Some(1)
        && string(field(message, c"operation")).as_deref() == Some(expected)
}
pub(super) fn new_message(operation: &CStr) -> Option<Message> {
    let result = Message::new(unsafe { xpc_dictionary_create(ptr::null(), ptr::null(), 0) })?;
    unsafe {
        xpc_dictionary_set_uint64(result.0, c"version".as_ptr(), 1);
        xpc_dictionary_set_string(result.0, c"operation".as_ptr(), operation.as_ptr());
    }
    Some(result)
}
pub(super) fn endpoint_id(message: Xpc) -> Option<[u8; 16]> {
    let value = field(message, c"endpoint_id");
    if !is_type(value, (&raw const _xpc_type_data).cast())
        || unsafe { xpc_data_get_length(value) } != 16
    {
        return None;
    }
    let pointer = unsafe { xpc_data_get_bytes_ptr(value) };
    if pointer.is_null() {
        return None;
    }
    let mut id = [0; 16];
    id.copy_from_slice(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), 16) });
    id.iter().any(|byte| *byte != 0).then_some(id)
}
fn entry_name(entry: &InventoryEntry) -> Option<&CStr> {
    let bytes =
        unsafe { std::slice::from_raw_parts(entry.display_name.as_ptr().cast::<u8>(), NAME_BYTES) };
    let value = CStr::from_bytes_until_nul(bytes).ok()?;
    (!value.is_empty() && value.to_str().is_ok()).then_some(value)
}
fn valid_backing(file: &File, size: usize) -> bool {
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
fn write_policy(message: Xpc, bundles: &[CString]) -> Option<()> {
    let array = Message::new(unsafe { xpc_array_create(ptr::null(), 0) })?;
    for bundle in bundles {
        let item = Message::new(unsafe { xpc_string_create(bundle.as_ptr()) })?;
        unsafe { xpc_array_append_value(array.0, item.0) };
    }
    unsafe { xpc_dictionary_set_value(message, c"transport_compatibility".as_ptr(), array.0) };
    Some(())
}
fn parse_policy(message: Xpc) -> Option<TransportCompatibility> {
    let value = field(message, c"transport_compatibility");
    let mut result = TransportCompatibility::default();
    if !is_type(value, (&raw const _xpc_type_array).cast()) {
        return None;
    }
    let count = unsafe { xpc_array_get_count(value) };
    if count > MAX_TRANSPORT_BUNDLES {
        return None;
    }
    let mut seen = Vec::with_capacity(count);
    for index in 0..count {
        let bundle = string(unsafe { xpc_array_get_value(value, index) })?;
        if !valid_bundle(bundle.to_str().ok()?) || seen.contains(&bundle) {
            return None;
        }
        for (target, byte) in result.bundles[index]
            .iter_mut()
            .zip(bundle.to_bytes_with_nul())
        {
            *target = c_char::from_ne_bytes([*byte]);
        }
        seen.push(bundle);
    }
    result.count = count;
    Some(result)
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

/// # Safety
/// A non-null ID must point to 16 readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_endpoint_remove(
    id: *const u8,
    generation: u64,
) -> Xpc {
    if id.is_null() || generation == 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(id, 16) };
    if bytes.iter().all(|byte| *byte == 0) {
        return ptr::null_mut();
    }
    let Some(message) = new_message(c"remove_endpoint") else {
        return ptr::null_mut();
    };
    unsafe {
        xpc_dictionary_set_data(message.0, c"endpoint_id".as_ptr(), id.cast(), 16);
        xpc_dictionary_set_uint64(message.0, c"generation".as_ptr(), generation);
    }
    message.into_raw()
}

/// # Safety
/// Message must be live XPC; outputs must be writable, distinct storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_endpoint_remove(
    message: Xpc,
    id: *mut u8,
    generation: *mut u64,
) -> bool {
    if id.is_null() || generation.is_null() || !operation(message, c"remove_endpoint") {
        return false;
    }
    let Some(parsed_id) = endpoint_id(message) else {
        return false;
    };
    let Some(parsed_generation) = uint(message, c"generation").filter(|value| *value != 0) else {
        return false;
    };
    unsafe {
        ptr::copy_nonoverlapping(parsed_id.as_ptr(), id, 16);
        generation.write(parsed_generation);
    }
    true
}

/// Creates a replacement intent, not a registry mutation. The Driver service must
/// compare the expected generation before retiring an endpoint's mapping.
///
/// # Safety
/// A non-null ID must point to 16 readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_create_transport_replacement_request(
    id: *const u8,
    expected_generation: u64,
) -> Xpc {
    if id.is_null() || expected_generation == 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(id, 16) };
    if bytes.iter().all(|byte| *byte == 0) {
        return ptr::null_mut();
    }
    let Some(message) = new_message(c"replace_transport") else {
        return ptr::null_mut();
    };
    unsafe {
        xpc_dictionary_set_data(message.0, c"endpoint_id".as_ptr(), id.cast(), 16);
        xpc_dictionary_set_uint64(
            message.0,
            c"expected_generation".as_ptr(),
            expected_generation,
        );
    }
    message.into_raw()
}

/// # Safety
/// Message must be live XPC; outputs must be writable, distinct storage. Invalid
/// messages leave both outputs unchanged. Generation policy belongs to the Driver service.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mr_hal_control_parse_transport_replacement_request(
    message: Xpc,
    id: *mut u8,
    expected_generation: *mut u64,
) -> bool {
    if id.is_null() || expected_generation.is_null() || !operation(message, c"replace_transport") {
        return false;
    }
    let Some(parsed_id) = endpoint_id(message) else {
        return false;
    };
    let Some(parsed_generation) = uint(message, c"expected_generation").filter(|value| *value != 0)
    else {
        return false;
    };
    unsafe {
        ptr::copy_nonoverlapping(parsed_id.as_ptr(), id, 16);
        expected_generation.write(parsed_generation);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    #[test]
    fn transport_policy_codec_is_required_bounded_and_transactional() {
        let message = message(&[]);
        unsafe {
            xpc_dictionary_set_value(
                message.0,
                c"transport_compatibility".as_ptr(),
                ptr::null_mut(),
            );
        };
        let mut output = InventorySnapshot::default();
        assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        write_policy(message.0, &[]).unwrap();
        assert!(unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        assert!(output.transport_compatibility.identifiers().is_empty());
        let valid = [c"com.test.App".to_owned(), c"net.test.Other".to_owned()];
        write_policy(message.0, &valid).unwrap();
        assert!(unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        let expected = output.transport_compatibility.identifiers();
        assert_eq!(expected, ["com.test.App", "net.test.Other"]);
        for malformed in [
            vec![c"".to_owned()],
            vec![c"com..app".to_owned()],
            vec![c"com.app*".to_owned()],
            vec![c"com.test.App".to_owned(); 2],
            vec![CString::new("a".repeat(NAME_BYTES)).unwrap()],
            (0..=MAX_TRANSPORT_BUNDLES)
                .map(|index| CString::new(format!("com.app{index}")).unwrap())
                .collect(),
        ] {
            write_policy(message.0, &malformed).unwrap();
            assert!(!unsafe {
                mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output)
            });
            assert_eq!(output.transport_compatibility.identifiers(), expected);
            assert_eq!(output.generation, 7);
        }
        unsafe { xpc_dictionary_set_uint64(message.0, c"transport_compatibility".as_ptr(), 1) };
        assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        let array = Message::new(unsafe { xpc_array_create(ptr::null(), 0) }).unwrap();
        let item = new_message(c"wrong_type").unwrap();
        unsafe {
            xpc_array_append_value(array.0, item.0);
            xpc_dictionary_set_value(message.0, c"transport_compatibility".as_ptr(), array.0);
        }
        assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        assert_eq!(output.transport_compatibility.identifiers(), expected);
        let policy = ProducerPolicy::new(&["com.service.App".into()]).unwrap();
        assert!(ProducerPolicy::new(&["com..bad".into()]).is_none());
        assert!(ProducerPolicy::new(&vec!["com.app".into(); MAX_TRANSPORT_BUNDLES + 1]).is_none());
        assert!(ProducerPolicy::new(&["com.app".into(), "com.app".into()]).is_none());
        // SAFETY: an empty inventory borrows no descriptors.
        let owned = Message::new(unsafe { create_inventory_snapshot(&[], 7, &policy) }).unwrap();
        assert_eq!(
            decode(&owned)
                .unwrap()
                .0
                .transport_compatibility
                .identifiers(),
            ["com.service.App"]
        );
        assert!(
            decode(&self::message(&[]))
                .unwrap()
                .0
                .transport_compatibility
                .identifiers()
                .is_empty()
        );
    }

    #[test]
    fn transport_policy_accepts_maximum_and_rejects_non_utf8() {
        let message = message(&[]);
        let valid: Vec<_> = (0..MAX_TRANSPORT_BUNDLES)
            .map(|index| CString::new(format!("com.app{index}")).unwrap())
            .collect();
        write_policy(message.0, &valid).unwrap();
        assert_eq!(
            decode(&message).unwrap().0.transport_compatibility.count,
            MAX_TRANSPORT_BUNDLES
        );
        write_policy(
            message.0,
            &[CString::new("a".repeat(NAME_BYTES - 1)).unwrap()],
        )
        .unwrap();
        assert!(decode(&message).is_some());
        write_policy(message.0, &[CString::new([0xff]).unwrap()]).unwrap();
        assert!(decode(&message).is_none());
    }

    fn backing() -> OwnedFd {
        let descriptor = crate::anonymous_memory::mr_shared_memory_create(4096, true);
        assert!(descriptor >= 0);
        let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let metadata = File::from(owned.try_clone().unwrap()).metadata().unwrap();
        assert_eq!(metadata.mode() & 0o170_000, 0, "Darwin POSIX shm type bits");
        assert!(metadata.len() >= 4096, "Darwin POSIX shm backing length");
        owned
    }

    fn entry(descriptor: &OwnedFd, index: u8, name: &str) -> InventoryEntry {
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

    fn message(entries: &[InventoryEntry]) -> Message {
        Message::new(unsafe {
            mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), 7)
        })
        .expect("valid inventory")
    }

    struct Snapshot(InventorySnapshot);
    impl Drop for Snapshot {
        fn drop(&mut self) {
            unsafe { mr_hal_control_inventory_snapshot_close(&raw mut self.0) };
        }
    }

    fn decode(message: &Message) -> Option<Snapshot> {
        let mut result = Snapshot(InventorySnapshot::default());
        unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut result.0) }
            .then_some(result)
    }

    fn item(message: &Message, index: usize) -> Xpc {
        unsafe { xpc_array_get_value(field(message.0, c"entries"), index) }
    }

    #[test]
    fn independent_names_ids_rates_and_descriptor_ownership_round_trip() {
        let source = backing();
        let mut second = entry(&source, 2, "방송 출력");
        second.sample_rate = 96_000;
        let message = message(&[entry(&source, 1, "Ableton"), second]);
        let parsed = decode(&message).unwrap();
        assert_eq!(parsed.0.count, 2);
        assert_eq!(parsed.0.generation, 7);
        assert_eq!(entry_name(&parsed.0.entries[0]), Some(c"Ableton"));
        assert_eq!(
            entry_name(&parsed.0.entries[1]).unwrap().to_str().unwrap(),
            "방송 출력"
        );
        assert_eq!(parsed.0.entries[1].sample_rate, 96_000);
        let owned = parsed.0.entries[0].descriptor;
        assert_ne!(owned, source.as_raw_fd());
        assert_ne!(owned, parsed.0.entries[1].descriptor);
        drop(message);
        drop(source);
        let borrowed = unsafe { BorrowedFd::borrow_raw(owned) };
        assert!(valid_backing(
            &File::from(borrowed.try_clone_to_owned().unwrap()),
            4096
        ));
    }

    #[test]
    fn empty_authoritative_inventory_and_close_are_valid() {
        let mut parsed = decode(&message(&[])).unwrap();
        assert_eq!(parsed.0.count, 0);
        assert_eq!(parsed.0.generation, 7);
        unsafe {
            mr_hal_control_inventory_snapshot_close(&raw mut parsed.0);
            mr_hal_control_inventory_snapshot_close(&raw mut parsed.0);
        }
        assert_eq!(parsed.0.generation, 0);
        assert!(parsed.0.entries.iter().all(|entry| entry.descriptor == -1));
    }

    #[test]
    fn invalid_creators_never_take_caller_descriptor() {
        let source = backing();
        let valid = entry(&source, 1, "Input");
        let mut invalid = valid;
        invalid.display_name = [c_char::from_ne_bytes([0xff]); NAME_BYTES];
        for entries in [&[valid, valid][..], &[invalid][..]] {
            assert!(
                unsafe {
                    mr_hal_control_create_inventory_snapshot(entries.as_ptr(), entries.len(), 1)
                }
                .is_null()
            );
        }
        assert!(
            unsafe { mr_hal_control_create_inventory_snapshot(ptr::null(), MAX_ENTRIES + 1, 1) }
                .is_null()
        );
        assert!(
            unsafe { mr_hal_control_create_inventory_snapshot(&raw const valid, 1, 0) }.is_null()
        );
        assert!(valid_backing(
            &File::from(source.try_clone().unwrap()),
            4096
        ));
    }

    #[test]
    fn regular_file_backing_is_rejected_by_creator_and_parser() {
        let fixture = mixrelay_test_support::FixtureDir::new("inventory-regular-backing")
            .expect("owned fixture directory");
        let file = File::create(fixture.join("backing")).expect("regular backing fixture");
        file.set_len(4096).expect("size regular backing fixture");
        let regular: OwnedFd = file.into();
        let invalid = entry(&regular, 1, "Regular file");
        // SAFETY: the entry borrows the live fixture FD for this synchronous call.
        assert!(
            unsafe { mr_hal_control_create_inventory_snapshot(&raw const invalid, 1, 1) }.is_null()
        );

        let source = backing();
        let message = message(&[entry(&source, 1, "Anonymous memory")]);
        // SAFETY: the message owns the dictionary; XPC duplicates the live fixture FD.
        unsafe {
            xpc_dictionary_set_fd(
                item(&message, 0),
                c"transport_fd".as_ptr(),
                regular.as_raw_fd(),
            );
        }
        unchanged_on_error(&message);
        assert_eq!(
            File::from(regular.try_clone().expect("caller still owns regular FD"))
                .metadata()
                .expect("fixture metadata")
                .len(),
            4096
        );
    }

    fn unchanged_on_error(message: &Message) {
        let mut output = InventorySnapshot {
            generation: 99,
            ..InventorySnapshot::default()
        };
        assert!(!unsafe { mr_hal_control_parse_inventory_snapshot(message.0, &raw mut output) });
        assert_eq!(output.generation, 99);
        assert_eq!(output.count, 0);
        assert!(output.entries.iter().all(|entry| entry.descriptor == -1));
    }

    #[test]
    fn rejects_duplicate_ids_without_partial_publication() {
        let source = backing();
        let message = message(&[entry(&source, 1, "One"), entry(&source, 2, "Two")]);
        unsafe {
            xpc_dictionary_set_data(
                item(&message, 1),
                c"endpoint_id".as_ptr(),
                [1_u8; 16].as_ptr().cast(),
                16,
            );
        };
        unchanged_on_error(&message);
    }

    #[test]
    fn malformed_final_entry_releases_previously_decoded_fds() {
        // Descriptor slots are process-global; isolate the leak check from
        // unrelated parallel tests which also open files.
        if std::env::var_os("MR_INVENTORY_FD_TEST_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "control_inventory::tests::malformed_final_entry_releases_previously_decoded_fds",
                    "--test-threads=1",
                ])
                .env("MR_INVENTORY_FD_TEST_CHILD", "1")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let source = backing();
        let message = message(&[entry(&source, 1, "One"), entry(&source, 2, "Two")]);
        // A backing length mismatch happens after both FD duplicates were made.
        unsafe { xpc_dictionary_set_uint64(item(&message, 1), c"mapped_size".as_ptr(), 1 << 20) };
        // dup returns the lowest available descriptor. Holding a sentinel means
        // a leaked parser FD consumes the next slot and changes the second dup.
        let probe = source.try_clone().unwrap();
        let next = source.try_clone().unwrap();
        let expected = next.as_raw_fd();
        drop(next);
        for _ in 0..100 {
            unchanged_on_error(&message);
            let next = source.try_clone().unwrap();
            assert_eq!(
                next.as_raw_fd(),
                expected,
                "partial parse leaked a descriptor"
            );
        }
        drop(probe);
    }

    #[test]
    fn strict_types_lengths_generation_and_frame_bound() {
        let source = backing();
        for key in [
            c"sample_rate",
            c"mapped_size",
            c"generation",
            c"transport_fd",
            c"display_name",
            c"endpoint_id",
        ] {
            let message = message(&[entry(&source, 1, "One")]);
            unsafe {
                if key == c"display_name" {
                    xpc_dictionary_set_uint64(item(&message, 0), key.as_ptr(), 3);
                } else {
                    xpc_dictionary_set_string(
                        item(&message, 0),
                        key.as_ptr(),
                        c"wrong type".as_ptr(),
                    );
                }
            };
            unchanged_on_error(&message);
        }
        let oversized = message(&[entry(&source, 1, "One")]);
        let array = field(oversized.0, c"entries");
        for _ in 0..MAX_ENTRIES {
            unsafe { xpc_array_append_value(array, item(&oversized, 0)) };
        }
        unchanged_on_error(&oversized);
        let generation = message(&[]);
        unsafe { xpc_dictionary_set_uint64(generation.0, c"generation".as_ptr(), 0) };
        unchanged_on_error(&generation);
    }

    #[test]
    fn requests_and_removes_are_typed_and_preserve_outputs_on_error() {
        let request = Message::new(mr_hal_control_create_inventory_request()).unwrap();
        assert!(unsafe { mr_hal_control_parse_inventory_request(request.0) });
        let id = [3_u8; 16];
        let remove =
            Message::new(unsafe { mr_hal_control_create_endpoint_remove(id.as_ptr(), 42) })
                .unwrap();
        assert!(!unsafe { mr_hal_control_parse_inventory_request(remove.0) });
        let mut parsed_id = [0; 16];
        let mut generation = 0;
        assert!(unsafe {
            mr_hal_control_parse_endpoint_remove(
                remove.0,
                parsed_id.as_mut_ptr(),
                &raw mut generation,
            )
        });
        assert_eq!(parsed_id, id);
        assert_eq!(generation, 42);
        unsafe { xpc_dictionary_set_uint64(remove.0, c"generation".as_ptr(), 0) };
        assert!(!unsafe {
            mr_hal_control_parse_endpoint_remove(
                remove.0,
                parsed_id.as_mut_ptr(),
                &raw mut generation,
            )
        });
        assert_eq!(parsed_id, id);
        assert_eq!(generation, 42);
        assert!(unsafe { mr_hal_control_create_endpoint_remove([0; 16].as_ptr(), 1) }.is_null());
        assert!(unsafe { mr_hal_control_create_endpoint_remove(id.as_ptr(), 0) }.is_null());
        assert!(Message::new(ptr::null_mut()).is_none());
    }

    #[test]
    fn replacement_request_round_trips_exact_identity_and_expected_generation() {
        let id = [7; 16];
        let request = Message::new(unsafe {
            mr_hal_control_create_transport_replacement_request(id.as_ptr(), u64::MAX)
        })
        .unwrap();
        let mut output = [0; 16];
        let mut generation = 0;
        assert!(unsafe {
            mr_hal_control_parse_transport_replacement_request(
                request.0,
                output.as_mut_ptr(),
                &raw mut generation,
            )
        });
        assert_eq!(output, id);
        assert_eq!(generation, u64::MAX);
        assert!(!unsafe { mr_hal_control_parse_inventory_request(request.0) });
        assert!(!unsafe {
            mr_hal_control_parse_endpoint_remove(
                request.0,
                output.as_mut_ptr(),
                &raw mut generation,
            )
        });
    }

    #[test]
    fn replacement_request_rejects_malformed_fields_without_output_changes() {
        let id = [8; 16];
        for case in 0..7 {
            let request = Message::new(unsafe {
                mr_hal_control_create_transport_replacement_request(id.as_ptr(), 1)
            })
            .unwrap();
            unsafe {
                match case {
                    0 => xpc_dictionary_set_uint64(request.0, c"version".as_ptr(), 2),
                    1 => xpc_dictionary_set_string(
                        request.0,
                        c"operation".as_ptr(),
                        c"remove_endpoint".as_ptr(),
                    ),
                    2 => xpc_dictionary_set_uint64(request.0, c"expected_generation".as_ptr(), 0),
                    3 => xpc_dictionary_set_string(
                        request.0,
                        c"expected_generation".as_ptr(),
                        c"1".as_ptr(),
                    ),
                    4 => xpc_dictionary_set_data(
                        request.0,
                        c"endpoint_id".as_ptr(),
                        id.as_ptr().cast(),
                        15,
                    ),
                    5 => xpc_dictionary_set_data(
                        request.0,
                        c"endpoint_id".as_ptr(),
                        [0_u8; 16].as_ptr().cast(),
                        16,
                    ),
                    _ => xpc_dictionary_set_value(
                        request.0,
                        c"expected_generation".as_ptr(),
                        ptr::null_mut(),
                    ),
                }
            }
            let mut output = [9; 16];
            let mut generation = 99;
            assert!(!unsafe {
                mr_hal_control_parse_transport_replacement_request(
                    request.0,
                    output.as_mut_ptr(),
                    &raw mut generation,
                )
            });
            assert_eq!(output, [9; 16]);
            assert_eq!(generation, 99);
        }
        assert!(
            unsafe { mr_hal_control_create_transport_replacement_request(id.as_ptr(), 0) }
                .is_null()
        );
        assert!(
            unsafe { mr_hal_control_create_transport_replacement_request([0; 16].as_ptr(), 1) }
                .is_null()
        );
        assert!(
            unsafe { mr_hal_control_create_transport_replacement_request(ptr::null(), 1) }
                .is_null()
        );
    }
}
