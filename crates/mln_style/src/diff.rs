//! What changed between two versions of a style's parts, which the renderer
//! reads to find what to redo.

use std::sync::Arc;

use mln_vec_map::VecMap;

use crate::layer_list::LayerList;

/// How a layer differs between two layer lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LayerChange {
    /// Only in the new list.
    Added,
    /// Only in the old list.
    Removed,
    /// In both, as another type: a removal and an addition.
    Replaced,
    /// In both, with other values that only need evaluating again.
    Restyled,
    /// In both, with other values that tiles are laid out with, so its
    /// source lays them out again.
    Relaid,
}

impl LayerChange {
    /// Whether the change needs the layer's tiles laid out again.
    #[must_use]
    pub fn relayouts(self) -> bool {
        matches!(self, LayerChange::Added | LayerChange::Replaced | LayerChange::Relaid)
    }

    /// Whether the layer leaves the old list's render layers.
    #[must_use]
    pub fn removes(self) -> bool {
        matches!(self, LayerChange::Removed | LayerChange::Replaced)
    }

    /// Whether the layer needs a new render layer.
    #[must_use]
    pub fn adds(self) -> bool {
        matches!(self, LayerChange::Added | LayerChange::Replaced)
    }

    /// Whether the layer keeps its render layer with new values.
    #[must_use]
    pub fn restyles(self) -> bool {
        matches!(self, LayerChange::Restyled | LayerChange::Relaid)
    }
}

/// The layers that differ between two layer lists, by id. A layer whose
/// style data is the same `Arc` in both lists has no entry.
#[derive(Clone, Debug, Default)]
pub struct LayerChanges {
    changes: VecMap<Arc<str>, LayerChange>,
    /// Whether the layers in both lists are in another order, or a layer was
    /// added or removed: the drawing order needs building again.
    pub order_changed: bool,
}

impl LayerChanges {
    /// What changed from `old` to `new`. A layer found in both is matched by
    /// id, wherever it is: one that only moved has no entry, and sets
    /// `order_changed`.
    #[must_use]
    pub fn between(old: &Arc<LayerList>, new: &Arc<LayerList>) -> LayerChanges {
        if Arc::ptr_eq(old, new) {
            return LayerChanges::default();
        }
        let mut changes: Vec<(Arc<str>, LayerChange)> = Vec::new();
        let mut order_changed = false;
        let mut previous_index = None;
        for layer in new.iter() {
            let Some((index, before)) = old.get_full(layer.get_id()) else {
                changes.push((Arc::from(layer.get_id()), LayerChange::Added));
                continue;
            };
            let (before, after) = (before.style(), layer.style());
            if before.info().name != after.info().name {
                changes.push((Arc::from(layer.get_id()), LayerChange::Replaced));
                continue;
            }
            order_changed |= previous_index.is_some_and(|previous| index < previous);
            previous_index = Some(index);
            if Arc::ptr_eq(before, after) {
                continue;
            }
            let change = if before.layout_difference(&**after) { LayerChange::Relaid } else { LayerChange::Restyled };
            changes.push((Arc::from(layer.get_id()), change));
        }
        for layer in old.iter() {
            if new.get(layer.get_id()).is_none() {
                changes.push((Arc::from(layer.get_id()), LayerChange::Removed));
            }
        }
        changes.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        let changes: VecMap<Arc<str>, LayerChange> = changes.into_iter().collect();
        let order_changed = order_changed || changes.values().any(|change| !change.restyles());
        LayerChanges { changes, order_changed }
    }

    /// The change of the layer `id`, if it changed.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<LayerChange> {
        self.changes.get(id).copied()
    }

    /// Each changed layer's id and change, in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&Arc<str>, LayerChange)> {
        self.changes.iter().map(|(id, change)| (id, *change))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}
