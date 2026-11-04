use super::*;

#[test]
fn requests_and_removes_are_typed_and_preserve_outputs_on_error() {
    let request = Message::new(mr_hal_control_create_inventory_request()).unwrap();
    assert!(unsafe { mr_hal_control_parse_inventory_request(request.0) });
    let id = [3_u8; 16];
    let remove =
        Message::new(unsafe { mr_hal_control_create_endpoint_remove(id.as_ptr(), 42) }).unwrap();
    assert!(!unsafe { mr_hal_control_parse_inventory_request(remove.0) });
    let mut parsed_id = [0; 16];
    let mut generation = 0;
    assert!(unsafe {
        mr_hal_control_parse_endpoint_remove(remove.0, parsed_id.as_mut_ptr(), &raw mut generation)
    });
    assert_eq!(parsed_id, id);
    assert_eq!(generation, 42);
    unsafe { xpc_dictionary_set_uint64(remove.0, c"generation".as_ptr(), 0) };
    assert!(!unsafe {
        mr_hal_control_parse_endpoint_remove(remove.0, parsed_id.as_mut_ptr(), &raw mut generation)
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
        mr_hal_control_parse_endpoint_remove(request.0, output.as_mut_ptr(), &raw mut generation)
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
        unsafe { mr_hal_control_create_transport_replacement_request(id.as_ptr(), 0) }.is_null()
    );
    assert!(
        unsafe { mr_hal_control_create_transport_replacement_request([0; 16].as_ptr(), 1) }
            .is_null()
    );
    assert!(
        unsafe { mr_hal_control_create_transport_replacement_request(ptr::null(), 1) }.is_null()
    );
}
