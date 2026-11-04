//! Rate and stream-format catalogs, setters and their publication.
use super::{global_address, set_global};
use crate::abi::OK;
use crate::properties::{
    Address,
    ffi::{mr_hal_core_property_get, mr_hal_core_property_set},
};
use crate::tests::Fixture;

fn catalog(fixture: &Fixture, object: u32, selector: [u8; 4], stride: usize, channels: u32) {
    let address = Address {
        selector: u32::from_be_bytes(selector),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let total = if stride == 16 { 6 } else { 24 };
    for count in 0..=total + 1 {
        let mut output = [0xa5_u8; 1400];
        let mut written = u32::MAX;
        assert_eq!(
            unsafe {
                mr_hal_core_property_get(
                    (&raw const *fixture.registry).cast_mut(),
                    object,
                    &raw const address,
                    0,
                    std::ptr::null(),
                    u32::try_from(count * stride).unwrap(),
                    &raw mut written,
                    output.as_mut_ptr().cast(),
                    0,
                )
            },
            0
        );
        let expected = count.min(total) * stride;
        assert_eq!(written as usize, expected);
        assert!(output[expected..].iter().all(|value| *value == 0xa5));
        for (index, entry) in output[..expected].chunks_exact(stride).enumerate() {
            let rate_index = if stride == 16 { index } else { index / 4 };
            let rate = f64::from(crate::formats::SAMPLE_RATES[rate_index]);
            assert_eq!(
                f64::from_ne_bytes(entry[..8].try_into().unwrap()).to_bits(),
                rate.to_bits()
            );
            if stride == 16 {
                assert_eq!(entry[..8], entry[8..16]);
            } else {
                let encoding = crate::properties::stream::NATIVE_FORMATS[index % 4];
                assert_eq!(
                    &entry[..40],
                    crate::properties::stream::format(
                        crate::formats::SAMPLE_RATES[rate_index],
                        channels,
                        encoding
                    )
                );
                assert_eq!(
                    u32::from_ne_bytes(entry[28..32].try_into().unwrap()),
                    channels
                );
                assert_eq!(
                    f64::from_ne_bytes(entry[40..48].try_into().unwrap()).to_bits(),
                    rate.to_bits()
                );
                assert_eq!(entry[40..48], entry[48..56]);
            }
        }
    }
}

#[test]
fn canonical_catalog_preserves_partial_read_bounds_for_both_stream_directions() {
    let fixture = Fixture::new();
    for channels in [1, 2, 7, 64] {
        let endpoint = u8::try_from(channels).unwrap();
        let id = fixture
            .registry
            .upsert(
                [endpoint; 16],
                "Catalog",
                48_000,
                (fixture.fd(endpoint, channels, channels), 24),
            )
            .unwrap();
        catalog(&fixture, id, *b"nsr#", 16, channels);
        catalog(&fixture, id + 1, *b"sfma", 56, channels);
        catalog(&fixture, id + 2, *b"sfma", 56, channels);
        catalog(&fixture, id + 1, *b"pfta", 56, channels);
        catalog(&fixture, id + 2, *b"pfta", 56, channels);
    }
}

#[test]
fn native_property_rate_setter_accepts_exact_catalog_only() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let address = Address {
        selector: u32::from_be_bytes(*b"nsrt"),
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    for &rate in crate::formats::SAMPLE_RATES {
        let value = f64::from(rate);
        assert_eq!(
            unsafe {
                mr_hal_core_property_set(
                    (&raw const *fixture.registry).cast_mut(),
                    id,
                    &raw const address,
                    8,
                    (&raw const value).cast(),
                )
            },
            0
        );
        assert_eq!(fixture.perform(), OK);
        assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, rate);
    }
    for value in [0.0, 48_000.5, 352_800.0, 384_000.0, f64::NAN, f64::INFINITY] {
        assert_ne!(
            unsafe {
                mr_hal_core_property_set(
                    (&raw const *fixture.registry).cast_mut(),
                    id,
                    &raw const address,
                    8,
                    (&raw const value).cast(),
                )
            },
            0
        );
        assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 192_000);
    }
}

#[test]
fn native_formats_publish_only_after_perform_and_remain_directional() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    for stream in [id + 1, id + 2] {
        for rate in crate::formats::SAMPLE_RATES {
            for encoding in crate::properties::stream::NATIVE_FORMATS {
                let before = fixture.property::<40>(stream, &global_address(*b"pft "));
                let requested = crate::properties::stream::format(*rate, 2, encoding);
                assert_eq!(set_global(&fixture, stream, *b"pft ", &requested), OK);
                assert_eq!(
                    fixture.property::<40>(stream, &global_address(*b"sfmt")),
                    before
                );
                assert_eq!(fixture.perform(), OK);
                assert_eq!(
                    &fixture.property::<40>(stream, &global_address(*b"pft "))[..],
                    requested
                );
                assert_eq!(
                    &fixture.property::<40>(stream, &global_address(*b"sfmt"))[..],
                    requested
                );
            }
        }
    }
    let snapshot = fixture.registry.snapshot(id).unwrap();
    assert_eq!(
        snapshot.capture_format,
        mixrelay_pcm_ring::SampleFormat::Signed32 as u32
    );
    assert_eq!(
        snapshot.render_format,
        mixrelay_pcm_ring::SampleFormat::Signed32 as u32
    );
}

#[test]
fn invalid_and_aborted_formats_preserve_prior_native_state() {
    let fixture = Fixture::new();
    let id = fixture.add(1, 48_000);
    let stream = id + 2;
    let before = fixture.property::<40>(stream, &global_address(*b"pft "));
    let valid =
        crate::properties::stream::format(96_000, 2, mixrelay_pcm_ring::SampleFormat::Signed24);
    for offset in [8, 12, 16, 20, 24, 28, 32, 36] {
        let mut invalid = valid.clone();
        invalid[offset] ^= 0x80;
        assert_ne!(set_global(&fixture, stream, *b"pft ", &invalid), OK);
        assert_eq!(
            fixture.property::<40>(stream, &global_address(*b"pft ")),
            before
        );
        assert!(crate::device::lock(&fixture.host.requests).is_empty());
    }
    assert_eq!(set_global(&fixture, stream, *b"sfmt", &valid), OK);
    let (_, generation) = fixture.pending();
    assert_eq!(
        fixture
            .registry
            .abort(id, crate::abi::CONFIG_ACTION, generation),
        OK
    );
    assert_eq!(fixture.perform(), OK);
    assert_eq!(
        fixture.property::<40>(stream, &global_address(*b"pft ")),
        before
    );
    assert_eq!(set_global(&fixture, stream, *b"pft ", &valid), OK);
    fixture
        .registry
        .upsert([1; 16], "Replacement", 48_000, (fixture.fd(1, 7, 3), 24))
        .unwrap();
    crate::tests::FAIL_REPLACE.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(fixture.perform(), crate::abi::NATIVE_ERROR);
    assert_eq!(
        fixture.property::<40>(stream, &global_address(*b"pft ")),
        before
    );
    assert_eq!(fixture.registry.snapshot(id).unwrap().sample_rate, 48_000);
}
