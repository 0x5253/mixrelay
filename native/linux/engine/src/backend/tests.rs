use super::*;
use crate::{
    runtime::{create_initial_processor, render_format},
    test_support::test_backend,
};
use mixrelay_engine_core::default_peak_meter_bank;

#[test]
fn initial_graph_and_native_inventory_are_empty_until_catalog_apply() {
    let document = GraphDocument::new(1, Vec::new(), Vec::new());
    let processor =
        create_initial_processor(&document, default_peak_meter_bank(), render_format()).unwrap();
    assert_eq!(processor.plan().revision(), document.revision);
    assert!(processor.plan().meter_bindings().is_empty());
    let backend = test_backend();
    let active = backend.active_spec.lock().unwrap();
    assert!(active.endpoints.is_empty());
    assert!(active.captures.is_empty());
    assert!(*active == ResourceSpec::from_document(&document).unwrap());
}
