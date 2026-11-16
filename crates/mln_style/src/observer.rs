// The observer methods take `&self`: the style holds an `Rc<dyn Observer>`,
// and the map's implementation uses interior mutability.

use mln_util::error::Error;

use crate::source::Source;

pub trait SourceObserver {
    fn on_source_loaded(&self, _source: &dyn Source) {}
    fn on_source_changed(&self, _source: &dyn Source) {}
    fn on_source_error(&self, _source: &dyn Source, _error: Error) {}
    fn on_source_description_changed(&self, _source: &dyn Source) {}
}

pub trait Observer: SourceObserver {
    fn on_update(&self) {}
    /// The style's projection changed: set or edited.
    fn on_projection_changed(&self) {}
}

/// The observer of a style that has none.
#[derive(Default)]
pub(crate) struct NullObserver;

impl SourceObserver for NullObserver {}
impl Observer for NullObserver {}
