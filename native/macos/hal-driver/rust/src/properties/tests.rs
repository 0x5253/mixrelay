//! Property catalog behavior through the C ABI against the crate's fake host.
mod audio_thread;
mod latency;
mod output_controls;
mod stream_formats;
mod transport;

use super::{Address, GLOBAL, ffi::mr_hal_core_property_set};
use crate::tests::Fixture;

fn global_address(selector: [u8; 4]) -> Address {
    Address {
        selector: u32::from_be_bytes(selector),
        scope: GLOBAL,
        element: 0,
    }
}

fn set_global(fixture: &Fixture, object: u32, selector: [u8; 4], bytes: &[u8]) -> i32 {
    let address = global_address(selector);
    // SAFETY: the fixture owns the registry; the input bytes live for the synchronous call.
    unsafe {
        mr_hal_core_property_set(
            (&raw const *fixture.registry).cast_mut(),
            object,
            &raw const address,
            u32::try_from(bytes.len()).unwrap(),
            bytes.as_ptr().cast(),
        )
    }
}
