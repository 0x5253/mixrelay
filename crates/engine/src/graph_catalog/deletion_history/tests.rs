use super::*;
use mixrelay_control_protocol::GraphCatalogAction;

fn catalog() -> Catalog {
    Catalog::imported(GraphDocument::new(1, vec![], vec![]))
}

#[test]
fn undo_retains_a_replacement_with_authored_name_and_preserves_background_selection_and_order() {
    let mut value = catalog();
    let original = value.selected_id().to_owned();
    value.delete_graph(&original, 1, 2).unwrap();
    value.graphs.active_mut().name = "New work".into();
    value.undo_deletion().unwrap();
    assert_eq!(value.selected_id(), original);
    assert_eq!(value.graphs.len(), 2);
    let replacement = value.graphs.iter().nth(1).unwrap().id.clone();
    value.graphs.select(&replacement);
    value.delete_graph(&original, 1, 3).unwrap();
    value.undo_deletion().unwrap();
    assert_eq!(value.selected_id(), replacement);
    assert_eq!(
        value
            .graphs
            .iter()
            .map(|graph| graph.id.clone())
            .collect::<Vec<_>>(),
        [original, replacement]
    );
}

#[test]
fn deletion_history_is_bounded_and_rejects_missing_or_colliding_targets() {
    let mut value = catalog();
    assert!(value.delete_graph("absent", 1, 2).is_err());
    for revision in 2..=102 {
        let id = value.selected_id().to_owned();
        value.delete_graph(&id, 1, revision).unwrap();
    }
    assert_eq!(value.deleted.len(), HISTORY_LIMIT);
    for _ in 0..HISTORY_LIMIT {
        value.undo_deletion().unwrap();
    }
    assert!(value.undo_deletion().is_err());
    assert_eq!(value.selected_id(), "graph-2");
    value
        .apply_action(GraphCatalogAction::RedoDelete, 1, 103)
        .unwrap();
    value.graphs.push_selected(SavedGraph::new(
        "graph-2".into(),
        "Collision".into(),
        GraphDocument::new(1, vec![], vec![]),
    ));
    assert!(value.undo_deletion().is_err());
}
