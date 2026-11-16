// C++ `Source` is an abstract class with data members (`baseImpl`,
// `observer`, `loaded`); the data is `SourceBase` and the virtual interface
// is the `Source` trait.
//
// Deviation (structural): C++ sources call their `SourceObserver*` (the
// owning style) from setters and from asynchronous request callbacks,
// which hold a `WeakPtr` to the source. A Rust callback cannot point into
// the map's sources, so a source keeps its mutable state
// behind interior mutability in an `Rc` (`SourceBase` fields are cells; the
// concrete sources are handles to an `Rc`'d inner state), the callbacks hold
// a `Weak` to it, and every source method takes `&self`. The map's observer
// (mln_map's style_sources.rs) then reads the changed `baseImpl` through the
// source and records it in the style.

use std::any::Any;
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_storage::FileSource;
use mln_style_types::SourceType;
use mln_util::logging::{Event, Log};

use crate::layer_type::SourceKind;
pub use crate::source_impl::{Impl, downcast_arc};

/// Hears about changes to a source that the source makes itself, from its
/// setters and from the answers to its requests. Methods take `&self`: a
/// source holds an `Rc<dyn SourceObserver>`.
pub trait SourceObserver {
    fn on_source_loaded(&self, _source: &dyn Source) {}
    fn on_source_changed(&self, _source: &dyn Source) {}
    fn on_source_error(&self, _source: &dyn Source, _error: mln_util::Error) {}
    fn on_source_description_changed(&self, _source: &dyn Source) {}
}

/// The kind of layer that a source of type `type_` feeds.
#[must_use]
pub fn layer_kind(type_: SourceType) -> SourceKind {
    match type_ {
        SourceType::Vector | SourceType::GeoJSON | SourceType::CustomVector | SourceType::CustomMVTVector => {
            SourceKind::Geometry
        }
        SourceType::Raster | SourceType::Image => SourceKind::Raster,
        SourceType::RasterDEM => SourceKind::RasterDem,
    }
}

/// The data members of `Source`.
pub struct SourceBase {
    base_impl: RefCell<Arc<dyn Impl>>,
    observer: RefCell<Option<Rc<dyn SourceObserver>>>,
    loaded: Cell<bool>,
}

impl std::fmt::Debug for SourceBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceBase")
            .field("base_impl", &self.base_impl.borrow())
            .field("loaded", &self.loaded.get())
            .finish_non_exhaustive()
    }
}

struct NullSourceObserver;
impl SourceObserver for NullSourceObserver {}

impl SourceBase {
    pub(crate) fn new(impl_: Arc<dyn Impl>) -> Self {
        SourceBase { base_impl: RefCell::new(impl_), observer: RefCell::new(None), loaded: Cell::new(false) }
    }

    fn impl_ref(&self) -> Ref<'_, Arc<dyn Impl>> {
        self.base_impl.borrow()
    }

    pub(crate) fn base_impl(&self) -> Arc<dyn Impl> {
        self.impl_ref().clone()
    }

    pub(crate) fn set_base_impl(&self, impl_: Arc<dyn Impl>) {
        *self.base_impl.borrow_mut() = impl_;
    }

    pub(crate) fn observer(&self) -> Rc<dyn SourceObserver> {
        match &*self.observer.borrow() {
            Some(observer) => Rc::clone(observer),
            None => Rc::new(NullSourceObserver),
        }
    }

    pub(crate) fn set_observer(&self, observer: Option<Rc<dyn SourceObserver>>) {
        *self.observer.borrow_mut() = observer;
    }

    pub(crate) fn loaded(&self) -> bool {
        self.loaded.get()
    }

    pub(crate) fn set_loaded(&self, loaded: bool) {
        self.loaded.set(loaded);
    }
}

pub trait Source: Any {
    fn base(&self) -> &SourceBase;

    /// Loads the source's description through `file_source`, if it has
    /// one to load. Its responses and background work answer on `run_loop`.
    fn load_description(&self, file_source: &dyn FileSource, run_loop: &mln_actor::RunLoop);

    /// A copy of the current impl.
    fn create_mutable(&self) -> Box<dyn Impl>;

    // Non-virtual members of the C++ class.

    /// The `baseImpl` member.
    fn base_impl(&self) -> Arc<dyn Impl> {
        self.base().base_impl()
    }

    /// The `loaded` member.
    fn loaded(&self) -> bool {
        self.base().loaded()
    }

    fn get_type(&self) -> SourceType {
        self.base_impl().type_
    }

    fn get_id(&self) -> String {
        self.base_impl().id.clone()
    }

    fn get_attribution(&self) -> Option<String> {
        self.base_impl().get_attribution()
    }

    fn is_volatile(&self) -> bool {
        self.base_impl().is_volatile()
    }

    fn set_observer(&self, observer: Option<Rc<dyn SourceObserver>>) {
        self.base().set_observer(observer);
    }

    fn get_prefetch_zoom_delta(&self) -> Option<u8> {
        self.base_impl().get_prefetch_zoom_delta()
    }

    fn get_minimum_tile_update_interval(&self) -> Duration {
        self.base_impl().get_minimum_tile_update_interval()
    }

    fn get_max_overscale_factor_for_parent_tiles(&self) -> Option<u8> {
        self.base_impl().get_max_overscale_factor_for_parent_tiles()
    }

    fn dump_debug_logs(&self) {
        Log::info(Event::General, format!("Source::id: {}", self.get_id()));
        Log::info(Event::General, format!("Source::loaded: {}", i32::from(self.loaded())));
    }
}

impl dyn Source {
    /// The concrete type behind this trait object, if it is a `T`.
    pub fn downcast_ref<T: Source>(&self) -> Option<&T> {
        let any: &dyn Any = self;
        any.downcast_ref()
    }
}

/// Whether `a` and `b` are the same source, rather than two that share an
/// ID: handles that clone one source are the same.
pub fn same_source(a: &dyn Source, b: &dyn Source) -> bool {
    std::ptr::eq(a.base(), b.base())
}

/// Replaces the impl with a mutated copy and notifies the observer, as the
/// C++ setters do (`createMutable()`, mutate, `baseImpl = ...`,
/// `observer->onSourceChanged(*this)`).
fn mutate_impl(source: &dyn Source, f: impl FnOnce(&mut dyn Impl)) {
    let mut new_impl = source.create_mutable();
    f(new_impl.as_mut());
    source.base().set_base_impl(Arc::from(new_impl));
    source.base().observer().on_source_changed(source);
}

pub fn set_volatile(source: &dyn Source, set: bool) {
    if source.is_volatile() == set {
        return;
    }
    mutate_impl(source, |impl_| impl_.base_mut().set_volatile(set));
}

pub fn set_prefetch_zoom_delta(source: &dyn Source, delta: Option<u8>) {
    if source.get_prefetch_zoom_delta() == delta {
        return;
    }
    mutate_impl(source, |impl_| impl_.base_mut().set_prefetch_zoom_delta(delta));
}

pub fn set_minimum_tile_update_interval(source: &dyn Source, interval: Duration) {
    if source.get_minimum_tile_update_interval() == interval {
        return;
    }
    mutate_impl(source, |impl_| impl_.base_mut().set_minimum_tile_update_interval(interval));
}

pub fn set_max_overscale_factor_for_parent_tiles(source: &dyn Source, overscale_factor: Option<u8>) {
    if source.get_max_overscale_factor_for_parent_tiles() == overscale_factor {
        return;
    }
    mutate_impl(source, |impl_| impl_.base_mut().set_max_overscale_factor_for_parent_tiles(overscale_factor));
}

// Debug is not a supertrait, for the reason at
// `impl Debug for dyn ErasedLayerStyle` (layer_type.rs).
impl std::fmt::Debug for dyn Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = self.base_impl();
        f.debug_struct("Source").field("id", &base.id).field("type", &base.type_).finish_non_exhaustive()
    }
}
