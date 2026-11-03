//! In-memory graph deletion history preserves authored identities and layout.
use super::{
    Catalog, GraphDocument, GraphLayout, MAX_GRAPHS, ProtocolError, SavedGraph, SavedGraphs,
    UNTITLED, invalid,
};

// Match the editor's bounded node history without retaining an unbounded catalog archive.
const HISTORY_LIMIT: usize = 100;

#[derive(Clone)]
pub(super) struct Deletion {
    pub(super) graph: SavedGraph,
    index: usize,
    selected: bool,
    replacement: Option<String>,
}

impl Catalog {
    pub(super) fn delete_graph(
        &mut self,
        id: &str,
        revision: u64,
        catalog_revision: u64,
    ) -> Result<(), ProtocolError> {
        let (index, graph) = self
            .graphs
            .iter()
            .enumerate()
            .find(|(_, graph)| graph.id == id)
            .map(|(index, graph)| (index, graph.clone()))
            .ok_or_else(|| invalid("graph does not exist"))?;
        let replacement_id = format!("graph-{catalog_revision}");
        let deletion = Deletion {
            graph,
            index,
            selected: self.selected_id() == id,
            replacement: (self.graphs.len() == 1).then(|| replacement_id.clone()),
        };
        self.graphs.remove(id, || {
            SavedGraph::new(
                replacement_id,
                UNTITLED.into(),
                GraphDocument::new(revision, vec![], vec![]),
            )
        });
        self.deleted.push(deletion);
        if self.deleted.len() > HISTORY_LIMIT {
            self.deleted.remove(0);
        }
        Ok(())
    }

    pub(super) fn undo_deletion(&mut self) -> Result<(), ProtocolError> {
        let deletion = self
            .deleted
            .pop()
            .ok_or_else(|| invalid("no graph deletion to undo"))?;
        if self
            .graphs
            .iter()
            .any(|graph| graph.id == deletion.graph.id)
        {
            return Err(invalid("graph identity collision"));
        }
        let selected = if deletion.selected {
            deletion.graph.id.clone()
        } else {
            self.selected_id().to_owned()
        };
        let mut graphs: Vec<_> = self
            .graphs
            .iter()
            .filter(|graph| {
                // Keep a replacement that has since acquired authored content.
                deletion.replacement.as_ref() != Some(&graph.id)
                    || !graph.document.nodes.is_empty()
                    || !graph.document.edges.is_empty()
                    || graph.name != UNTITLED
                    || graph.layout != GraphLayout::default()
            })
            .cloned()
            .collect();
        if graphs.len() >= MAX_GRAPHS {
            return Err(invalid("catalog full"));
        }
        graphs.insert(deletion.index.min(graphs.len()), deletion.graph.clone());
        self.graphs = SavedGraphs::from_list(graphs, &selected)
            .ok_or_else(|| invalid("invalid restored selection"))?;
        self.restored.push(deletion);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
