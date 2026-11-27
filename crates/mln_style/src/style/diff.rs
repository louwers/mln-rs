//! What changed between two versions of a style, which the renderer reads
//! to find what to redo: [`StyleDiff`].

use std::cmp::Ordering;
use std::sync::Arc;

use crate::collections::VecMap;
use crate::image;
use crate::layer::LayerList;
use crate::source::{Source, SourceList};
use crate::style::Style;

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

/// The items, by id, that differ between two id-sorted lists of shared
/// values: sources or images.
pub struct ItemChanges<V> {
    /// Only in the new list, or in both as another kind (a source of
    /// another type).
    pub added: VecMap<String, V>,
    /// Only in the old list, or in both as another kind.
    pub removed: VecMap<String, V>,
    /// The new values of the items in both whose values differ.
    pub changed: VecMap<String, V>,
}

impl<V> Default for ItemChanges<V> {
    fn default() -> Self {
        ItemChanges { added: VecMap::new(), removed: VecMap::new(), changed: VecMap::new() }
    }
}

impl<V: Clone> ItemChanges<V> {
    /// Merges `old` and `new`, both sorted by id with unique ids. An item
    /// whose id is in both but that `same_kind` tells apart is removed and
    /// added; one that `same` tells apart is changed.
    fn between(
        old: &[V],
        new: &[V],
        id: impl Fn(&V) -> &str,
        same_kind: impl Fn(&V, &V) -> bool,
        same: impl Fn(&V, &V) -> bool,
    ) -> ItemChanges<V> {
        let (mut removed, mut added, mut changed) = (Vec::new(), Vec::new(), Vec::new());
        let (mut old_items, mut new_items) = (old.iter().peekable(), new.iter().peekable());
        loop {
            let order = match (old_items.peek(), new_items.peek()) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(before), Some(after)) => id(before).cmp(id(after)),
            };
            match order {
                Ordering::Less => removed.extend(old_items.next()),
                Ordering::Greater => added.extend(new_items.next()),
                Ordering::Equal => {
                    let (Some(before), Some(after)) = (old_items.next(), new_items.next()) else { break };
                    if !same_kind(before, after) {
                        removed.push(before);
                        added.push(after);
                    } else if !same(before, after) {
                        changed.push(after);
                    }
                }
            }
        }
        // Each list is in id order, which the maps keep.
        let map = |items: Vec<&V>| items.into_iter().map(|item| (id(item).to_owned(), item.clone())).collect();
        ItemChanges { added: map(added), removed: map(removed), changed: map(changed) }
    }
}

impl<V> ItemChanges<V> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// The sources of `list` in id order.
#[must_use]
pub fn sources_by_id(list: &SourceList) -> Vec<Source> {
    let mut sources: Vec<_> = list.iter().cloned().collect();
    sources.sort_unstable_by(|a, b| a.id().cmp(b.id()));
    sources
}

/// What a renderer has to redo to go from one version of a style to
/// another: the change class of each layer, source and image.
///
/// It compares the two versions, not the edits between them, so it answers
/// for however many versions came in between. Parts whose `Arc` is the same
/// in both come out empty without a walk. Light, sky and global state
/// aren't in it: a renderer compares them by their own pointers.
#[derive(Default)]
pub struct StyleDiff {
    pub layers: LayerChanges,
    /// A source of another type with the same id is removed and added.
    pub sources: ItemChanges<Source>,
    pub images: ItemChanges<Arc<image::Impl>>,
}

impl StyleDiff {
    /// What changed from `old` to `new`. `None` is a renderer with nothing
    /// yet, for which every layer, source and image of `new` is added.
    #[must_use]
    pub fn between(old: Option<&Style>, new: &Style) -> StyleDiff {
        let empty_layers = Arc::default();
        let old_layers = old.map_or(&empty_layers, Style::get_layer_list);
        let layers = LayerChanges::between(old_layers, new.get_layer_list());

        let sources = match old {
            Some(old) if Arc::ptr_eq(old.get_sources(), new.get_sources()) => ItemChanges::default(),
            _ => ItemChanges::between(
                &old.map(|old| sources_by_id(old.get_sources())).unwrap_or_default(),
                &sources_by_id(new.get_sources()),
                Source::id,
                |before, after| before.info().name == after.info().name,
                Source::ptr_eq,
            ),
        };

        let images = match old {
            Some(old) if Arc::ptr_eq(old.get_images(), new.get_images()) => ItemChanges::default(),
            _ => ItemChanges::between(
                old.map_or(&[][..], |old| old.get_images().as_slice()),
                new.get_images(),
                |image| &image.id,
                |_, _| true,
                Arc::ptr_eq,
            ),
        };

        StyleDiff { layers, sources, images }
    }
}
