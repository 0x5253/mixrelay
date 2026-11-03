//! Maps committed HAL state changes to SDK property notifications.
use crate::properties::{Address, GLOBAL, INPUT, OUTPUT};

#[repr(C)]
pub(crate) struct Notification {
    object: u32,
    address: Address,
}

/// Writes the property addresses changed by one control-plane event, in host order.
/// Returns the count fitting `capacity`; a null output only queries that count.
///
/// # Safety
/// A non-null output must point to `capacity` aligned writable notifications and
/// remain exclusively borrowed for this call. No output storage is retained.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mr_hal_core_notifications(
    object: u32,
    change: u32,
    output: *mut Notification,
    capacity: usize,
) -> usize {
    use crate::abi::{
        ALIVE, FORMAT, INPUT_LATENCY, LIST, NAME, OUTPUT_MUTE, OUTPUT_VOLUME, RUNNING,
        STREAM_ACTIVE, TRANSPORT,
    };
    let mut count = 0;
    let mut push = |object, selector, scope| {
        if count == capacity {
            return;
        }
        if !output.is_null() {
            // SAFETY: count is below the caller's writable capacity; each index
            // is initialized once and no reference to the ABI output is retained.
            unsafe {
                output.add(count).write(Notification {
                    object,
                    address: Address {
                        selector: u32::from_be_bytes(selector),
                        scope,
                        element: 0,
                    },
                });
            }
        }
        count += 1;
    };
    match change {
        LIST => {
            push(1, *b"dev#", GLOBAL);
            push(1, *b"ownd", GLOBAL);
        }
        NAME => {
            for id in [object, object + 1, object + 2] {
                push(id, *b"lnam", GLOBAL);
            }
        }
        FORMAT => {
            push(object, *b"nsrt", GLOBAL);
            push(object, *b"ownd", GLOBAL);
            push(object, *b"ctrl", GLOBAL);
            push(object, *b"ctrl", OUTPUT);
            for scope in [INPUT, OUTPUT] {
                push(object, *b"stm#", scope);
                push(object, *b"srnd", scope);
            }
            for stream in [object + 1, object + 2] {
                for selector in [*b"sfmt", *b"pft ", *b"sfma", *b"pfta"] {
                    push(stream, selector, GLOBAL);
                }
            }
        }
        INPUT_LATENCY => push(object, *b"ltnc", INPUT),
        TRANSPORT => push(object, *b"tran", GLOBAL),
        ALIVE => push(object, *b"livn", GLOBAL),
        RUNNING => push(object, *b"goin", GLOBAL),
        STREAM_ACTIVE => push(object, *b"sact", GLOBAL),
        OUTPUT_VOLUME => {
            push(object + 3, *b"lcsv", GLOBAL);
            push(object + 3, *b"lcdv", GLOBAL);
        }
        OUTPUT_MUTE => push(object + 4, *b"bcvl", GLOBAL),
        _ => {}
    }
    count
}

#[cfg(test)]
mod tests;
