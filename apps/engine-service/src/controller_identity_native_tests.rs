//! Native composition issues an independent incarnation for every engine owner.
use super::*;
#[test]
fn each_engine_run_has_its_own_incarnation() {
    let (first, second) = (new_incarnation(), new_incarnation());
    assert_ne!(first, second);
    assert_eq!(first.len(), 32);
    assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
    let engine = crate::endpoint_settings_tests::controller(default_duplex_document(1, None));
    assert_eq!(ControlBackend::incarnation(&*engine), engine.incarnation);
}
