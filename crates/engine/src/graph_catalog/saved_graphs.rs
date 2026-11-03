//! Ordered saved graphs that always contain exactly one selected graph.
//!
//! The selected graph is stored by value between the graphs before and after it, so
//! selection cannot name a missing graph and the list can never become empty.

use std::iter;

use super::SavedGraph;

#[derive(Clone)]
pub(super) struct SavedGraphs {
    before: Vec<SavedGraph>,
    active: SavedGraph,
    after: Vec<SavedGraph>,
}

impl SavedGraphs {
    pub(super) fn single(graph: SavedGraph) -> Self {
        Self {
            before: Vec::new(),
            active: graph,
            after: Vec::new(),
        }
    }

    /// Splits a saved list at its selected graph. Returns `None` when no graph has `active_id`.
    pub(super) fn from_list(mut graphs: Vec<SavedGraph>, active_id: &str) -> Option<Self> {
        let index = graphs.iter().position(|graph| graph.id == active_id)?;
        let active = graphs.remove(index);
        let after = graphs.split_off(index);
        Some(Self {
            before: graphs,
            active,
            after,
        })
    }

    pub(super) fn active(&self) -> &SavedGraph {
        &self.active
    }

    pub(super) fn active_mut(&mut self) -> &mut SavedGraph {
        &mut self.active
    }

    pub(super) fn len(&self) -> usize {
        self.before.len() + 1 + self.after.len()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &SavedGraph> {
        self.before
            .iter()
            .chain(iter::once(&self.active))
            .chain(&self.after)
    }

    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = &mut SavedGraph> {
        self.before
            .iter_mut()
            .chain(iter::once(&mut self.active))
            .chain(&mut self.after)
    }

    /// Appends `graph` at the end of the list and selects it.
    pub(super) fn push_selected(&mut self, graph: SavedGraph) {
        let previous = std::mem::replace(&mut self.active, graph);
        self.before.push(previous);
        self.before.append(&mut self.after);
    }

    /// Selects the graph with `id` without changing the list order. Returns whether it exists.
    pub(super) fn select(&mut self, id: &str) -> bool {
        if self.active.id == id {
            return true;
        }
        if let Some(index) = self.before.iter().position(|graph| graph.id == id) {
            let target = self.before.remove(index);
            let moved = self.before.split_off(index);
            let previous = std::mem::replace(&mut self.active, target);
            self.after
                .splice(0..0, moved.into_iter().chain(iter::once(previous)));
            return true;
        }
        if let Some(index) = self.after.iter().position(|graph| graph.id == id) {
            let target = self.after.remove(index);
            let rest = self.after.split_off(index);
            let previous = std::mem::replace(&mut self.active, target);
            self.before.push(previous);
            self.before.append(&mut self.after);
            self.after = rest;
            return true;
        }
        false
    }

    /// Removes the graph with `id`. A removed selection moves to the next graph, or to the
    /// previous one at the end of the list; removing the last graph selects `replacement()`.
    /// Returns whether the graph existed.
    pub(super) fn remove(&mut self, id: &str, replacement: impl FnOnce() -> SavedGraph) -> bool {
        if let Some(index) = self.before.iter().position(|graph| graph.id == id) {
            self.before.remove(index);
            return true;
        }
        if let Some(index) = self.after.iter().position(|graph| graph.id == id) {
            self.after.remove(index);
            return true;
        }
        if self.active.id != id {
            return false;
        }
        self.active = if self.after.is_empty() {
            self.before.pop().unwrap_or_else(replacement)
        } else {
            self.after.remove(0)
        };
        true
    }
    #[cfg(test)]
    pub(super) fn push(&mut self, graph: SavedGraph) {
        self.after.push(graph);
    }
}

#[cfg(test)]
mod tests;
