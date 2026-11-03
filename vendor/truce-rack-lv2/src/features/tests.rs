use super::*;

#[test]
fn uri_ids_and_reverse_lookup_belong_to_one_instance() {
    let first = UridMap::new();
    let second = UridMap::new();
    let first_uri = c"urn:mixrelay:first";
    let second_uri = c"urn:mixrelay:second";
    let first_id = first.map_uri(first_uri);
    let second_id = second.map_uri(second_uri);
    assert_eq!(first_id, second_id);
    assert_eq!(first.uri(first_id).as_deref(), Some("urn:mixrelay:first"));
    assert_eq!(
        second.uri(second_id).as_deref(),
        Some("urn:mixrelay:second")
    );
    assert_eq!(first.map_uri(first_uri), first_id);
    assert_ne!(first.map_uri(second_uri), first_id);
    assert_eq!(second.uri(first.map_uri(second_uri)), None);
}

#[test]
fn callback_rejects_missing_context_or_uri_without_mutating_the_map() {
    let mut map = UridMap::new();
    assert_eq!(
        map_callback(std::ptr::null_mut(), c"urn:mixrelay:test".as_ptr()),
        0
    );
    assert_eq!(map_callback((&raw mut map).cast(), std::ptr::null()), 0);
    assert_eq!(map.uri(1), None);
    assert_eq!(
        map_callback((&raw mut map).cast(), c"urn:mixrelay:test".as_ptr()),
        1
    );
}
