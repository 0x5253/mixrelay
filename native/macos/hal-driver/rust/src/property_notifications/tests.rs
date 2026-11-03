use super::*;
use crate::abi::{FORMAT, LIST, NAME};

const OBJECT: u32 = 20;
const SENTINEL: (u32, u32, u32, u32) = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
type NotificationPayload = (usize, [(u32, u32, u32, u32); 16]);

fn payload(change: u32, capacity: usize) -> NotificationPayload {
    let mut output: [Notification; 16] = std::array::from_fn(|_| Notification {
        object: SENTINEL.0,
        address: Address {
            selector: SENTINEL.1,
            scope: SENTINEL.2,
            element: SENTINEL.3,
        },
    });
    assert!(capacity <= output.len());
    // SAFETY: this initialized ABI array is aligned, exclusive and large enough.
    let count = unsafe { mr_hal_core_notifications(OBJECT, change, output.as_mut_ptr(), capacity) };
    (
        count,
        output.map(|value| {
            (
                value.object,
                value.address.selector,
                value.address.scope,
                value.address.element,
            )
        }),
    )
}

#[test]
fn format_notifications_preserve_host_order_and_partial_output_bounds() {
    let expected = [
        (OBJECT, *b"nsrt", GLOBAL),
        (OBJECT, *b"ownd", GLOBAL),
        (OBJECT, *b"ctrl", GLOBAL),
        (OBJECT, *b"ctrl", OUTPUT),
        (OBJECT, *b"stm#", INPUT),
        (OBJECT, *b"srnd", INPUT),
        (OBJECT, *b"stm#", OUTPUT),
        (OBJECT, *b"srnd", OUTPUT),
        (OBJECT + 1, *b"sfmt", GLOBAL),
        (OBJECT + 1, *b"pft ", GLOBAL),
        (OBJECT + 1, *b"sfma", GLOBAL),
        (OBJECT + 1, *b"pfta", GLOBAL),
        (OBJECT + 2, *b"sfmt", GLOBAL),
        (OBJECT + 2, *b"pft ", GLOBAL),
        (OBJECT + 2, *b"sfma", GLOBAL),
        (OBJECT + 2, *b"pfta", GLOBAL),
    ]
    .map(|(object, selector, scope)| (object, u32::from_be_bytes(selector), scope, 0));
    for capacity in [0, 5, 16] {
        let (count, output) = payload(FORMAT, capacity);
        assert_eq!(count, capacity.min(expected.len()));
        assert_eq!(output[..count], expected[..count]);
        assert!(output[count..].iter().all(|value| *value == SENTINEL));
        // SAFETY: null output queries the fitting count and is never dereferenced.
        assert_eq!(
            unsafe { mr_hal_core_notifications(OBJECT, FORMAT, std::ptr::null_mut(), capacity) },
            count
        );
    }
}

#[test]
fn inventory_and_rename_notifications_target_owned_objects() {
    let (count, inventory) = payload(LIST, 16);
    assert_eq!(count, 2);
    assert_eq!(
        inventory[..count],
        [
            (1, u32::from_be_bytes(*b"dev#"), GLOBAL, 0),
            (1, u32::from_be_bytes(*b"ownd"), GLOBAL, 0),
        ]
    );
    let (count, rename) = payload(NAME, 16);
    assert_eq!(count, 3);
    assert_eq!(
        rename[..count],
        [OBJECT, OBJECT + 1, OBJECT + 2].map(|object| (
            object,
            u32::from_be_bytes(*b"lnam"),
            GLOBAL,
            0
        ))
    );
    let (count, unsupported) = payload(u32::MAX, 16);
    assert_eq!(count, 0);
    assert_eq!(unsupported, [SENTINEL; 16]);
}

#[test]
fn transport_notifications_target_the_device_global_property() {
    let (count, output) = payload(crate::abi::TRANSPORT, 16);
    assert_eq!(count, 1);
    assert_eq!(output[0], (OBJECT, u32::from_be_bytes(*b"tran"), GLOBAL, 0));
    assert!(output[1..].iter().all(|value| *value == SENTINEL));
}

#[test]
fn native_output_notifications_target_level_and_mute_control_objects() {
    let (count, volume) = payload(crate::abi::OUTPUT_VOLUME, 16);
    assert_eq!(count, 2);
    assert_eq!(
        volume[..count],
        [*b"lcsv", *b"lcdv"].map(|selector| (OBJECT + 3, u32::from_be_bytes(selector), GLOBAL, 0))
    );
    let (count, mute) = payload(crate::abi::OUTPUT_MUTE, 16);
    assert_eq!(count, 1);
    assert_eq!(
        mute[0],
        (OBJECT + 4, u32::from_be_bytes(*b"bcvl"), GLOBAL, 0)
    );
}
