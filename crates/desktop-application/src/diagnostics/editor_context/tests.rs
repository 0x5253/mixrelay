use super::*;

fn parse(value: Value) -> Result<EditorContext, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn keeps_problem_codes_and_scan_counts() {
    let context = parse(json!({
        "problems": ["graph_apply_timeout", "inventory_failed"],
        "processorScan": [{"format": "vst3", "stage": "probe", "count": 18}],
    }))
    .unwrap();
    assert_eq!(
        context.to_report(),
        json!({
            "problems": ["graph_apply_timeout", "inventory_failed"],
            "processorScan": [{"format": "vst3", "stage": "probe", "count": 18}],
        })
    );
}

#[test]
fn refuses_free_text_names_and_unbounded_payloads() {
    for problems in [
        json!(["/Users/name/Library"]),
        json!(["Example Effect"]),
        json!([""]),
        json!(["x".repeat(CODE_LIMIT + 1)]),
        json!(vec!["code"; PROBLEM_LIMIT + 1]),
    ] {
        assert!(parse(json!({"problems": problems, "processorScan": []})).is_err());
    }
    assert!(
        parse(json!({
            "problems": [],
            "processorScan": [{"format": "vst3", "stage": "probe", "count": 1, "plugin": "SECRET"}],
        }))
        .is_err()
    );
    assert!(parse(json!({"problems": [], "processorScan": [], "graph": "SECRET"})).is_err());
    assert!(
        parse(json!({
            "problems": [],
            "processorScan": vec![json!({"format": "clap", "stage": "scan", "count": 1}); SCAN_GROUP_LIMIT + 1],
        }))
        .is_err()
    );
}
