//! Runtime input latencies carried by every v1 inventory snapshot.
use super::{
    _xpc_type_array, MAX_ENTRIES, Message, Xpc, dictionary, endpoint_id, field, is_type,
    new_message, ptr, uint, xpc_array_append_value, xpc_array_create, xpc_array_get_count,
    xpc_array_get_value, xpc_dictionary_set_data, xpc_dictionary_set_uint64,
    xpc_dictionary_set_value,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputLatency {
    pub endpoint_id: [u8; 16],
    pub rate: u32,
    pub engine_pid: u32,
    pub lease: u64,
    pub revision: u64,
    pub frames: Option<u32>,
}

/// Sets the runtime latency list of an inventory message, replacing any previous list.
/// # Safety
/// `message` is a live mutable XPC dictionary exclusively owned by the caller.
pub unsafe fn write_input_latencies(message: Xpc, entries: &[InputLatency]) -> bool {
    if entries.len() > MAX_ENTRIES || !dictionary(message) {
        return false;
    }
    let Some(array) = Message::new(unsafe { xpc_array_create(ptr::null(), 0) }) else {
        return false;
    };
    for entry in entries {
        let Some(item) = new_message(c"latency") else {
            return false;
        };
        unsafe {
            xpc_dictionary_set_data(
                item.0,
                c"endpoint_id".as_ptr(),
                entry.endpoint_id.as_ptr().cast(),
                16,
            );
            for (key, value) in [
                (c"rate", u64::from(entry.rate)),
                (c"engine_pid", u64::from(entry.engine_pid)),
                (c"lease", entry.lease),
                (c"revision", entry.revision),
                (c"known", u64::from(entry.frames.is_some())),
                (c"frames", u64::from(entry.frames.unwrap_or(0))),
            ] {
                xpc_dictionary_set_uint64(item.0, key.as_ptr(), value);
            }
            xpc_array_append_value(array.0, item.0);
        }
    }
    unsafe { xpc_dictionary_set_value(message, c"input_latencies".as_ptr(), array.0) };
    true
}

/// Parses the required bounded runtime latency list transactionally.
/// # Safety
/// `message` is a live XPC dictionary retained for this call.
pub unsafe fn parse_input_latencies(message: Xpc) -> Option<Vec<InputLatency>> {
    if !dictionary(message) {
        return None;
    }
    let array = field(message, c"input_latencies");
    if !is_type(array, (&raw const _xpc_type_array).cast()) {
        return None;
    }
    let count = unsafe { xpc_array_get_count(array) };
    if count > MAX_ENTRIES {
        return None;
    }
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let item = unsafe { xpc_array_get_value(array, index) };
        if !dictionary(item) {
            return None;
        }
        let entry = InputLatency {
            endpoint_id: endpoint_id(item)?,
            rate: u32::try_from(uint(item, c"rate")?)
                .ok()
                .filter(|rate| *rate != 0)?,
            engine_pid: u32::try_from(uint(item, c"engine_pid")?)
                .ok()
                .filter(|pid| *pid != 0 && i32::try_from(*pid).is_ok())?,
            lease: uint(item, c"lease").filter(|lease| *lease != 0)?,
            revision: uint(item, c"revision")?,
            frames: match uint(item, c"known")? {
                0 => {
                    if uint(item, c"frames")? != 0 {
                        return None;
                    }
                    None
                }
                1 => Some(u32::try_from(uint(item, c"frames")?).ok()?),
                _ => return None,
            },
        };
        if result
            .iter()
            .any(|old: &InputLatency| old.endpoint_id == entry.endpoint_id)
        {
            return None;
        }
        result.push(entry);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_inventory::xpc_dictionary_set_string;
    #[test]
    fn latency_list_is_required_typed_bounded_and_preserves_unknown() {
        let message = new_message(c"inventory_snapshot").unwrap();
        assert!(unsafe { parse_input_latencies(message.0) }.is_none());
        assert!(unsafe { write_input_latencies(message.0, &[]) });
        assert_eq!(unsafe { parse_input_latencies(message.0) }, Some(vec![]));
        let entry = InputLatency {
            endpoint_id: [1; 16],
            rate: 48_000,
            engine_pid: 2,
            lease: 3,
            revision: 4,
            frames: Some(0),
        };
        for frames in [Some(0), Some(257), None] {
            let entry = InputLatency { frames, ..entry };
            assert!(unsafe { write_input_latencies(message.0, &[entry]) });
            assert_eq!(
                unsafe { parse_input_latencies(message.0) },
                Some(vec![entry])
            );
        }
        assert!(unsafe { write_input_latencies(message.0, &[entry, entry]) });
        assert!(unsafe { parse_input_latencies(message.0) }.is_none());
        for bad in [
            InputLatency { rate: 0, ..entry },
            InputLatency {
                engine_pid: 0,
                ..entry
            },
            InputLatency { lease: 0, ..entry },
        ] {
            assert!(unsafe { write_input_latencies(message.0, &[bad]) });
            assert!(unsafe { parse_input_latencies(message.0) }.is_none());
        }
        assert!(!unsafe { write_input_latencies(message.0, &[entry; MAX_ENTRIES + 1]) });
        unsafe {
            xpc_dictionary_set_string(message.0, c"input_latencies".as_ptr(), c"bad".as_ptr());
        };
        assert!(unsafe { parse_input_latencies(message.0) }.is_none());
    }
}
