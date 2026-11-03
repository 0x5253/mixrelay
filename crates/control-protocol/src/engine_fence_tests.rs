#[test]
fn invalid_operation_id_is_a_stable_wire_error() {
    let code = super::ErrorCode::InvalidOperationId;
    let value = serde_json::to_value(code).unwrap();
    assert_eq!(value, "invalid_operation_id");
    assert_eq!(
        serde_json::from_value::<super::ErrorCode>(value).unwrap(),
        code
    );
}

#[test]
fn engine_replaced_is_a_stable_wire_error() {
    let error = super::ProtocolError {
        code: super::ErrorCode::EngineReplaced,
        parameters: serde_json::json!({"operationOutcome":"not_admitted"}),
    };
    let value = serde_json::to_value(&error).unwrap();
    assert_eq!(value["code"], "engine_replaced");
    assert_eq!(
        serde_json::from_value::<super::ProtocolError>(value).unwrap(),
        error
    );
}
