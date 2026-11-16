// The observer methods take `&self`: the style holds an `Rc<dyn Observer>`,
// and the map's implementation uses interior mutability.

pub trait Observer {
    fn on_update(&self) {}
    /// The style's projection changed: set or edited.
    fn on_projection_changed(&self) {}
}

/// The observer of a style that has none.
#[derive(Default)]
pub(crate) struct NullObserver;

impl Observer for NullObserver {}
