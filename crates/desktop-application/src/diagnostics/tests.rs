use super::*;
fn projected(
    driver: Option<&DriverSnapshot>,
    runtime: Option<&Value>,
    editor: Option<&EditorContext>,
    os_version: &str,
) -> Value {
    report(
        driver,
        runtime,
        editor,
        &ReportEnvironment {
            build_version: "0.1.0",
            platform: "macos",
            architecture: "aarch64",
            os_version,
            created_unix_seconds: Some(42),
        },
    )
}
#[test]
fn export_is_useful_when_runtime_is_unavailable_and_never_embeds_errors() {
    let value = projected(
        None,
        Some(&json!({"status":503,"body":{"reason":"SECRET"}})),
        None,
        "27.0.1",
    );
    assert_eq!(value["runtime"]["available"], false);
    assert_eq!(value["driver"]["available"], false);
    assert!(!value.to_string().contains("SECRET"));
}

#[test]
fn positive_schema_keeps_counters_but_excludes_private_and_unknown_payloads() {
    let response = json!({"status":200,"body":{
        "underruns":8,"sampleRate":48000,"password":"SECRET", "graph":{"name":"SECRET"},
        "sampleFaults":{"sourceSamples":2,"inputSamples":3,"outputSamples":4,"blocks":1,"samples":["SECRET"],"nodeId":"SECRET"},
        "audioSession":{"discontinuities":3,"processingState":"SECRET", "issue":{"path":"SECRET"},
            "virtualEndpointSettings":{"devices":[{"name":"SECRET"}]},"format":{"sampleRate":48000,"blockFrames":128,"path":"SECRET"}}
    }});
    let value = projected(None, Some(&response), None, "27.0.1");
    assert_eq!(value["runtime"]["underruns"], 8);
    assert_eq!(
        value["runtime"]["sampleFaults"],
        json!({"sourceSamples":2,"inputSamples":3,"outputSamples":4,"blocks":1})
    );
    assert_eq!(value["runtime"]["audioSession"]["discontinuities"], 3);
    assert_eq!(
        value["runtime"]["audioSession"]["format"]["blockFrames"],
        128
    );
    assert!(!value.to_string().contains("SECRET"));
}

#[test]
fn editor_context_is_included_only_from_the_diagnostics_window() {
    let context: EditorContext = serde_json::from_value(json!({
        "problems": ["graph_apply_timeout"],
        "processorScan": [{"format": "clap", "stage": "scan", "count": 2}],
    }))
    .unwrap();
    let window = projected(None, None, Some(&context), "27.0.1");
    assert_eq!(window["editor"]["problems"], json!(["graph_apply_timeout"]));
    assert_eq!(window["editor"]["processorScan"][0]["count"], 2);
    assert_eq!(projected(None, None, None, "27.0.1")["editor"], Value::Null);
}

#[test]
fn export_reservation_is_owner_scoped_and_released_on_cancellation() {
    let first = Arc::new(ExportAdmission::default());
    let other = Arc::new(ExportAdmission::default());
    let permit = first.try_start().unwrap();
    assert!(first.exporting());
    assert!(first.try_start().is_none());
    assert!(other.try_start().is_some());
    drop(permit);
    assert!(!first.exporting());
    assert!(first.try_start().is_some());
}
#[test]
fn report_uses_the_supplied_timestamp_and_host_identity() {
    let value = projected(None, None, None, "27.0.1");
    assert_eq!(value["createdUnixSeconds"], 42);
    assert_eq!(value["platform"], "macos");
    assert_eq!(value["osVersion"], "27.0.1");
}
