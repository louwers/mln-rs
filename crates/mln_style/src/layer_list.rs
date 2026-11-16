use std::sync::Arc;

use indexmap::IndexMap;
use mln_util::hash::RandomState;

use crate::layer::Layer;

/// A style's layers in drawing order, bottom first, found by id. Ids are
/// unique.
#[derive(Clone, Default)]
pub struct LayerList(IndexMap<Arc<str>, Layer, RandomState>);

impl LayerList {
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Layer> {
        self.0.get(id)
    }

    /// The layer `id` and its place in drawing order.
    #[must_use]
    pub fn get_full(&self, id: &str) -> Option<(usize, &Layer)> {
        self.0.get_full(id).map(|(index, _, layer)| (index, layer))
    }

    /// The place of the layer `id` in drawing order.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.0.get_index_of(id)
    }

    /// The layers, bottom first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Layer> + ExactSizeIterator {
        self.0.values()
    }

    /// The bottom layer.
    #[must_use]
    pub fn first(&self) -> Option<&Layer> {
        self.0.first().map(|(_, layer)| layer)
    }

    /// Adds `layer` below the layer `before`, or on top when `before` is
    /// `None` or not in the list. Gives the layer back if one has its id.
    pub(crate) fn insert_before(&mut self, layer: Layer, before: Option<&str>) -> Result<(), Layer> {
        if self.0.contains_key(layer.get_id()) {
            return Err(layer);
        }
        let index = before.and_then(|before| self.0.get_index_of(before)).unwrap_or(self.0.len());
        self.0.shift_insert(index, Arc::from(layer.get_id()), layer);
        Ok(())
    }

    /// Adds `layers` on top, in order. Their ids must differ from each other
    /// and from the list's.
    pub(crate) fn extend(&mut self, layers: impl IntoIterator<Item = Layer>) {
        for layer in layers {
            // `shift_insert` at the end is `insert`, through the code
            // `insert_before` already needs.
            let previous = self.0.shift_insert(self.0.len(), Arc::from(layer.get_id()), layer);
            debug_assert!(previous.is_none(), "duplicate layer id");
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<Layer> {
        self.0.shift_remove(id)
    }

    /// Moves the layer `id` below the layer `before`, or on top when
    /// `before` is `None`, `id` itself or not in the list. Returns false when
    /// the list has no layer `id`.
    pub(crate) fn move_before(&mut self, id: &str, before: Option<&str>) -> bool {
        let Some(from) = self.0.get_index_of(id) else { return false };
        let to = match before.filter(|before| *before != id).and_then(|before| self.0.get_index_of(before)) {
            Some(before) if before > from => before - 1,
            Some(before) => before,
            None => self.0.len() - 1,
        };
        self.0.move_index(from, to);
        true
    }

    /// Replaces the layer at `index`, which must have the same id.
    pub(crate) fn replace(&mut self, index: usize, layer: Layer) {
        let Some((id, slot)) = self.0.get_index_mut(index) else {
            debug_assert!(false, "no layer at {index}");
            return;
        };
        debug_assert_eq!(&**id, layer.get_id());
        *slot = layer;
    }
}

impl std::fmt::Debug for LayerList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for &'a LayerList {
    type Item = &'a Layer;
    type IntoIter = indexmap::map::Values<'a, Arc<str>, Layer>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.values()
    }
}
