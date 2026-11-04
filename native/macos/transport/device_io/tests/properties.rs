use super::*;

#[test]
fn property_admission_observes_rates_and_rejects_malformed_stream_readback() {
    let io = owner(true, 2);
    let binding = NativeBinding {
        device: 123,
        offset: 0,
        total: 0,
    };
    for rate in [44100_u32, 96000] {
        assert_eq!(
            native::format(
                &io.selection,
                binding,
                &PropertiesStub {
                    rate: f64::from(rate),
                    ..PropertiesStub::default()
                }
            ),
            Some(rate)
        );
    }
    for rate in [f64::NAN, 48000.5, 0.0] {
        assert_eq!(
            native::format(
                &io.selection,
                binding,
                &PropertiesStub {
                    rate,
                    ..PropertiesStub::default()
                }
            ),
            None
        );
    }
    for stream_size in [0, 3, 8] {
        assert_eq!(
            native::format(
                &io.selection,
                binding,
                &PropertiesStub {
                    stream_size,
                    ..PropertiesStub::default()
                }
            ),
            None
        );
    }
}

#[test]
fn default_endpoint_changes_and_unproven_transport_identity_are_rejected() {
    for input in [false, true] {
        let io = DeviceIo::for_test(c"", input, 2, 48000).expect("owner");
        let properties = PropertiesStub::default();
        // SAFETY: sole test control owner accesses connection state before registration.
        unsafe {
            (*io.control.get()).connection.binding.device = 123;
            assert!(!control::alive(&io, &properties));
        }
        assert_eq!(properties.default_queries.get(), 1);
        assert_eq!(native::clock_device(123, &properties), 123);
        for transport in [Some(0), Some(u32::from_be_bytes(*b"grup")), None] {
            assert_eq!(
                native::clock_device(
                    123,
                    &PropertiesStub {
                        transport,
                        ..PropertiesStub::default()
                    }
                ),
                0
            );
        }
        // SAFETY: no registration means no proven clock regardless of native transport.
        assert_eq!(unsafe { control::clock_device(&io, &properties) }, 0);
        let mut registration = RegistrationStub::default();
        assert_eq!(register(&io, &mut registration), 0);
        // SAFETY: test control queries are serialized; mock registration retains this owner.
        assert_eq!(unsafe { control::clock_device(&io, &properties) }, 123);
        for transport in [Some(0), Some(u32::from_be_bytes(*b"grup")), None] {
            let observed = PropertiesStub {
                transport,
                ..PropertiesStub::default()
            };
            // SAFETY: only the retained control cell and mock properties are observed.
            assert_eq!(unsafe { control::clock_device(&io, &observed) }, 0);
        }
        // SAFETY: mock native drain precedes releasing the test owner.
        assert!(unsafe {
            (*io.control.get()).lifecycle.disconnect(&mut StubBackend {
                io: &io,
                native: &mut registration,
            })
        });
    }
}
