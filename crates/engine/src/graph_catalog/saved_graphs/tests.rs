use super::*;
use mixrelay_audio_graph::GraphDocument;

fn graph(id: &str) -> SavedGraph {
    SavedGraph::new(id.into(), id.into(), GraphDocument::new(1, vec![], vec![]))
}

fn list(ids: &[&str], active: &str) -> SavedGraphs {
    SavedGraphs::from_list(ids.iter().map(|id| graph(id)).collect(), active).unwrap()
}

fn ids(graphs: &SavedGraphs) -> Vec<&str> {
    graphs.iter().map(|graph| graph.id.as_str()).collect()
}

#[test]
fn a_saved_list_without_its_selected_graph_is_rejected() {
    assert!(SavedGraphs::from_list(vec![graph("a")], "missing").is_none());
    assert!(SavedGraphs::from_list(Vec::new(), "a").is_none());
}

#[test]
fn selection_keeps_list_order_in_both_directions() {
    let mut graphs = list(&["a", "b", "c", "d"], "c");
    assert!(graphs.select("a"));
    assert_eq!(graphs.active().id, "a");
    assert_eq!(ids(&graphs), ["a", "b", "c", "d"]);
    assert!(graphs.select("d"));
    assert_eq!(graphs.active().id, "d");
    assert_eq!(ids(&graphs), ["a", "b", "c", "d"]);
    assert!(!graphs.select("missing"));
    assert_eq!(graphs.active().id, "d");
}

#[test]
fn created_graphs_are_appended_and_selected() {
    let mut graphs = list(&["a", "b", "c"], "a");
    graphs.push_selected(graph("e"));
    assert_eq!(graphs.active().id, "e");
    assert_eq!(ids(&graphs), ["a", "b", "c", "e"]);
    assert_eq!(graphs.len(), 4);
}

#[test]
fn removing_the_selection_moves_to_the_next_then_previous_graph() {
    let mut graphs = list(&["a", "b", "c"], "b");
    assert!(graphs.remove("b", || unreachable!("graphs remain")));
    assert_eq!(graphs.active().id, "c");
    assert!(graphs.remove("c", || unreachable!("graphs remain")));
    assert_eq!(graphs.active().id, "a");
    assert!(graphs.remove("a", || graph("new")));
    assert_eq!(ids(&graphs), ["new"]);
    assert!(!graphs.remove("missing", || unreachable!("nothing removed")));
}

#[test]
fn removing_another_graph_keeps_the_selection() {
    let mut graphs = list(&["a", "b", "c"], "b");
    assert!(graphs.remove("a", || unreachable!("graphs remain")));
    assert!(graphs.remove("c", || unreachable!("graphs remain")));
    assert_eq!(ids(&graphs), ["b"]);
}
