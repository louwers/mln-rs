use std::sync::Arc;

use crate::collections::VecMap;
use crate::layer::Layer;

/// A style's layers in drawing order, bottom first, found by id. Ids are
/// unique. A style version holds its list behind an `Arc`, which an edit
/// copies and replaces, so a list that a version shares never changes.
#[derive(Clone, Default)]
pub struct LayerList {
    layers: Vec<Layer>,
    /// Each layer's place in `layers`, by id.
    places: VecMap<Arc<str>, usize>,
}

impl LayerList {
    #[must_use]
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Layer> {
        self.get_full(id).map(|(_, layer)| layer)
    }

    /// The layer `id` and its place in drawing order.
    #[must_use]
    pub fn get_full(&self, id: &str) -> Option<(usize, &Layer)> {
        let index = self.index_of(id)?;
        self.layers.get(index).map(|layer| (index, layer))
    }

    /// The place of the layer `id` in drawing order.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.places.get(id).copied()
    }

    /// The layers, bottom first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Layer> + ExactSizeIterator {
        self.layers.iter()
    }

    /// The bottom layer.
    #[must_use]
    pub fn first(&self) -> Option<&Layer> {
        self.layers.first()
    }

    /// Records the places of the layers from `from` on.
    fn renumber(&mut self, from: usize) {
        for (index, layer) in self.layers.iter().enumerate().skip(from) {
            if let Some(place) = self.places.get_mut(layer.get_id()) {
                *place = index;
            }
        }
    }

    /// Adds `layer` below the layer `before`, or on top when `before` is
    /// `None` or not in the list. Gives the layer back if one has its id.
    pub(crate) fn insert_before(&mut self, layer: Layer, before: Option<&str>) -> Result<(), Layer> {
        if self.places.contains_key(layer.get_id()) {
            return Err(layer);
        }
        let index = before.and_then(|before| self.index_of(before)).unwrap_or(self.layers.len());
        self.places.insert(Arc::from(layer.get_id()), index);
        self.layers.insert(index, layer);
        self.renumber(index + 1);
        Ok(())
    }

    /// Adds `layers` on top, in order. Their ids must differ from each other
    /// and from the list's.
    pub(crate) fn extend(&mut self, layers: impl IntoIterator<Item = Layer>) {
        for layer in layers {
            let previous = self.places.insert(Arc::from(layer.get_id()), self.layers.len());
            debug_assert!(previous.is_none(), "duplicate layer id");
            self.layers.push(layer);
        }
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<Layer> {
        let index = self.places.remove(id)?;
        let layer = self.layers.remove(index);
        self.renumber(index);
        Some(layer)
    }

    /// Moves the layer `id` below the layer `before`, or on top when
    /// `before` is `None`, `id` itself or not in the list. Returns false when
    /// the list has no layer `id`.
    pub(crate) fn move_before(&mut self, id: &str, before: Option<&str>) -> bool {
        let Some(from) = self.index_of(id) else { return false };
        let to = match before.filter(|before| *before != id).and_then(|before| self.index_of(before)) {
            Some(before) if before > from => before - 1,
            Some(before) => before,
            None => self.layers.len() - 1,
        };
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.renumber(from.min(to));
        true
    }

    /// Replaces the layer at `index`, which must have the same id.
    pub(crate) fn replace(&mut self, index: usize, layer: Layer) {
        let Some(slot) = self.layers.get_mut(index) else {
            debug_assert!(false, "no layer at {index}");
            return;
        };
        debug_assert_eq!(slot.get_id(), layer.get_id());
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
    type IntoIter = std::slice::Iter<'a, Layer>;

    fn into_iter(self) -> Self::IntoIter {
        self.layers.iter()
    }
}
