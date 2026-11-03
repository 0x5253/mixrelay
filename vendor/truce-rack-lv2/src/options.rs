//! The standard LV2 Options feature, cached for one native instance.

use std::ffi::{CStr, c_void};

use truce_rack_core::error::{Error, Result};

use crate::features::UridMap;

const OPTIONS_URI: &[u8] = b"http://lv2plug.in/ns/ext/options#options\0";
const MIN_BLOCK_URI: &[u8] = b"http://lv2plug.in/ns/ext/buf-size#minBlockLength\0";
const MAX_BLOCK_URI: &[u8] = b"http://lv2plug.in/ns/ext/buf-size#maxBlockLength\0";
const NOMINAL_BLOCK_URI: &[u8] = b"http://lv2plug.in/ns/ext/buf-size#nominalBlockLength\0";
const SAMPLE_RATE_URI: &[u8] = b"http://lv2plug.in/ns/ext/parameters#sampleRate\0";
const ATOM_INT_URI: &[u8] = lv2_raw::atom::LV2_ATOM__INT;
const ATOM_FLOAT_URI: &[u8] = lv2_raw::atom::LV2_ATOM__FLOAT;
const INSTANCE_CONTEXT: u32 = 0;
const SCALAR_BYTES: u32 = 4;

/// The C layout of `LV2_Options_Option` from the official Options ABI.
#[repr(C)]
#[derive(Clone, Copy)]
struct OptionValue {
    context: u32,
    subject: u32,
    key: u32,
    size: u32,
    value_type: u32,
    value: *const c_void,
}

impl OptionValue {
    const TERMINATOR: Self = Self {
        context: 0,
        subject: 0,
        key: 0,
        size: 0,
        value_type: 0,
        value: std::ptr::null(),
    };

    fn instance(key: u32, value_type: u32, value: *const c_void) -> Self {
        Self {
            context: INSTANCE_CONTEXT,
            subject: 0,
            key,
            size: SCALAR_BYTES,
            value_type,
            value,
        }
    }
}

/// Heap-stable host-owned values and the zero-terminated options array.
///
/// Lilv receives `data()` as the Options feature while this box remains alive;
/// no callback or audio-thread code mutates these values after instantiation.
pub(super) struct HostOptions {
    min_block: i32,
    max_block: i32,
    nominal_block: i32,
    sample_rate: f32,
    entries: [OptionValue; 5],
}

impl HostOptions {
    /// Map standard option and atom URIs once and allocate stable native storage.
    pub(super) fn new(map: &UridMap) -> Box<Self> {
        let key = |uri: &[u8]| {
            // SAFETY: each constant is NUL-terminated for the entire process lifetime.
            map.map_uri(unsafe { CStr::from_ptr(uri.as_ptr().cast()) })
        };
        let keys = [
            key(MIN_BLOCK_URI),
            key(MAX_BLOCK_URI),
            key(NOMINAL_BLOCK_URI),
            key(SAMPLE_RATE_URI),
        ];
        let atom_int = key(ATOM_INT_URI);
        let atom_float = key(ATOM_FLOAT_URI);
        let mut host = Box::new(Self {
            min_block: 1,
            max_block: 1,
            nominal_block: 1,
            sample_rate: 1.0,
            entries: [OptionValue::TERMINATOR; 5],
        });
        // The box fixes the addresses before their pointers are embedded in the array.
        host.entries[0] = OptionValue::instance(
            keys[0],
            atom_int,
            std::ptr::from_ref(&host.min_block).cast(),
        );
        host.entries[1] = OptionValue::instance(
            keys[1],
            atom_int,
            std::ptr::from_ref(&host.max_block).cast(),
        );
        host.entries[2] = OptionValue::instance(
            keys[2],
            atom_int,
            std::ptr::from_ref(&host.nominal_block).cast(),
        );
        host.entries[3] = OptionValue::instance(
            keys[3],
            atom_float,
            std::ptr::from_ref(&host.sample_rate).cast(),
        );
        host
    }

    /// Set the actual instance rate and the maximum DSP `run()` frame count.
    ///
    /// The host can send shorter, non-power-of-two blocks. `nominalBlockLength` is only
    /// a hint, not a fixed-block guarantee. Values are checked before f32/i32 conversion.
    pub(super) fn configure(&mut self, rate: f64, maximum_block: usize) -> Result<()> {
        if !rate.is_finite() || rate <= 0.0 || rate > f64::from(f32::MAX) {
            return Err(Error::Other("invalid LV2 Options sample rate".into()));
        }
        let block = i32::try_from(maximum_block)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| Error::Other("invalid LV2 Options maximum block length".into()))?;
        let sample_rate = rate as f32;
        if sample_rate <= 0.0 {
            return Err(Error::Other("LV2 Options sample rate underflow".into()));
        }
        self.sample_rate = sample_rate;
        self.max_block = block;
        self.nominal_block = block;
        // `configure` has an exclusive borrow of the owner. Publish fresh field
        // pointers after updating it; callers configure only while no native
        // instance can still read the previous Options array.
        self.entries[0].value = std::ptr::from_ref(&self.min_block).cast();
        self.entries[1].value = std::ptr::from_ref(&self.max_block).cast();
        self.entries[2].value = std::ptr::from_ref(&self.nominal_block).cast();
        self.entries[3].value = std::ptr::from_ref(&self.sample_rate).cast();
        Ok(())
    }

    /// Pointer for the `LV2_OPTIONS__options` feature's `void* data` field.
    /// The plugin must not write through this pointer, per the LV2 Options contract.
    pub(super) fn data(&self) -> *mut c_void {
        self.entries.as_ptr().cast_mut().cast()
    }

    /// Maximum frame count promised to the current native instance.
    pub(super) fn maximum_block(&self) -> usize {
        // Construction and configure guarantee a positive i32, representable on the
        // supported 32/64-bit hosts. Reading the audio bound cannot fail or panic.
        usize::try_from(self.max_block).expect("validated LV2 maximum block must be positive")
    }
}

/// Standard feature URI; used by the owner when assembling its `LV2_Feature` array.
pub(super) const FEATURE_URI: &[u8] = OPTIONS_URI;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_have_official_types_values_and_terminator() {
        let map = UridMap::new();
        let mut options = HostOptions::new(&map);
        let address = options.data();
        options.configure(48_000.0, 512).unwrap();
        assert_eq!(options.data(), address);
        assert_eq!(options.maximum_block(), 512);
        assert_eq!(options.min_block, 1);
        assert_eq!(options.max_block, 512);
        assert_eq!(options.nominal_block, 512);
        assert_eq!(options.sample_rate.to_bits(), 48_000.0_f32.to_bits());
        assert_eq!(options.data().cast_const(), options.entries.as_ptr().cast());
        let key = |uri: &[u8]| map.map_uri(unsafe { CStr::from_ptr(uri.as_ptr().cast()) });
        let int_type = key(ATOM_INT_URI);
        let float_type = key(ATOM_FLOAT_URI);
        for (entry, expected) in options.entries[..4].iter().zip([
            key(MIN_BLOCK_URI),
            key(MAX_BLOCK_URI),
            key(NOMINAL_BLOCK_URI),
            key(SAMPLE_RATE_URI),
        ]) {
            assert_eq!(entry.context, INSTANCE_CONTEXT);
            assert_eq!(entry.subject, 0);
            assert_eq!(entry.key, expected);
            assert_eq!(entry.size, SCALAR_BYTES);
            assert!(!entry.value.is_null());
        }
        assert!(
            options.entries[..3]
                .iter()
                .all(|entry| entry.value_type == int_type)
        );
        assert_eq!(options.entries[3].value_type, float_type);
        assert_eq!(
            options.entries[0].value,
            std::ptr::from_ref(&options.min_block).cast()
        );
        assert_eq!(
            options.entries[1].value,
            std::ptr::from_ref(&options.max_block).cast()
        );
        assert_eq!(
            options.entries[2].value,
            std::ptr::from_ref(&options.nominal_block).cast()
        );
        assert_eq!(
            options.entries[3].value,
            std::ptr::from_ref(&options.sample_rate).cast()
        );
        assert_eq!(options.entries[4].key, 0);
        assert!(options.entries[4].value.is_null());
    }

    #[test]
    fn rejects_invalid_configuration_without_changing_published_values() {
        let map = UridMap::new();
        let mut options = HostOptions::new(&map);
        options.configure(44_100.0, 256).unwrap();
        for rate in [
            0.0,
            -1.0,
            f64::MIN_POSITIVE,
            f64::NAN,
            f64::INFINITY,
            f64::MAX,
        ] {
            assert!(options.configure(rate, 512).is_err());
        }
        for block in [0, usize::MAX] {
            assert!(options.configure(96_000.0, block).is_err());
        }
        assert_eq!(options.sample_rate.to_bits(), 44_100.0_f32.to_bits());
        assert_eq!(options.maximum_block(), 256);
    }
}
