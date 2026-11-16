use crate::source_type::Source;

/// A style's sources in the order they were added, found by id. Ids are
/// unique. A change to a source replaces its value, so two lists whose
/// sources are the same by [`Source::ptr_eq`] hold the same sources.
///
/// A style has few sources, so a lookup is a scan.
#[derive(Clone, Default)]
pub struct SourceList(Vec<Source>);

impl SourceList {
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.0.iter().position(|source| source.id() == id)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Source> {
        self.0.iter().find(|source| source.id() == id)
    }

    /// The sources, in the order they were added.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Source> + ExactSizeIterator {
        self.0.iter()
    }

    /// Replaces the source with `source`'s id in place, or adds it last.
    pub(crate) fn set(&mut self, source: Source) {
        match self.position(source.id()) {
            Some(index) => self.0[index] = source,
            None => self.0.push(source),
        }
    }

    /// Removes the source `id`, keeping the others in order.
    pub(crate) fn remove(&mut self, id: &str) -> Option<Source> {
        self.position(id).map(|index| self.0.remove(index))
    }
}

impl std::fmt::Debug for SourceList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(Source::id)).finish()
    }
}
