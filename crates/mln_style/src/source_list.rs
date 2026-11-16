use std::sync::Arc;

use indexmap::IndexMap;
use mln_util::hash::RandomState;

use crate::source;

/// A style's sources in the order they were added, found by id. Ids are
/// unique. Each source is its shared record, which a change to the source
/// replaces, so two lists whose records are the same pointer hold the same
/// source.
#[derive(Clone, Default)]
pub struct SourceList(IndexMap<Arc<str>, Arc<dyn source::Impl>, RandomState>);

impl SourceList {
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Arc<dyn source::Impl>> {
        self.0.get(id)
    }

    /// The sources, in the order they were added.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Arc<dyn source::Impl>> + ExactSizeIterator {
        self.0.values()
    }

    /// Replaces the source with `source`'s id in place, or adds it last.
    pub(crate) fn set(&mut self, source: Arc<dyn source::Impl>) {
        match self.0.get_mut(source.id.as_str()) {
            Some(slot) => *slot = source,
            None => {
                self.0.insert(Arc::from(source.id.as_str()), source);
            }
        }
    }

    /// Removes the source `id`, keeping the others in order.
    pub(crate) fn remove(&mut self, id: &str) -> Option<Arc<dyn source::Impl>> {
        self.0.shift_remove(id)
    }
}

impl std::fmt::Debug for SourceList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}
