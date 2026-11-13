// Ports include/mln/style/sources/custom_vector_source.hpp,
// src/mln/style/sources/custom_vector_source.cpp and
// src/mln/style/sources/custom_vector_source_impl.{hpp,cpp}, with ffi
// patches 0051, 0052 and 0075.
//
// `CustomVectorSource` is a handle (see source.rs): `Clone` shares the
// source. As for a custom geometry source, the host answers its tile
// functions through a `Send` [`CustomVectorSourceHandle`].

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::scheduler;
use mln_storage::{Data, FileSource};
use mln_style_types::SourceType;
use mln_tile_id::CanonicalTileID;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::immutable::Immutable;
use mln_util::range::Range;

use crate::custom_tile_loader::{CustomTileLoader, TileFunction};
use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, static_immutable_cast};
use crate::source_impl::{self, ImplBase};

/// `style::TileDataFormat`: the encoding of a custom vector tile's bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TileDataFormat {
    /// Mapbox Vector Tile.
    #[default]
    Mvt,
    /// MapLibre Tile, decoded with the map's FastPFOR option.
    Mlt,
}

/// The data a custom vector tile gets: its bytes, if any, and their format.
pub type CustomVectorTileData = (Option<Data>, TileDataFormat);

/// The loader of a custom vector source.
pub type CustomVectorTileLoader = CustomTileLoader<CustomVectorTileData>;

/// `CustomVectorSource::Options`.
#[derive(Clone)]
pub struct Options {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomVectorSourceHandle::set_tile_data`].
    pub fetch_tile_function: Option<TileFunction>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile_function: Option<TileFunction>,
    pub zoom_range: Range<u8>,
}

impl Default for Options {
    fn default() -> Self {
        Options { fetch_tile_function: None, cancel_tile_function: None, zoom_range: Range::new(0, 18) }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("fetch_tile_function", &self.fetch_tile_function.is_some())
            .field("cancel_tile_function", &self.cancel_tile_function.is_some())
            .field("zoom_range", &self.zoom_range)
            .finish()
    }
}

/// A `Send` handle to a custom vector source's loader, for the host to
/// answer its tile functions from any thread. It does nothing once the
/// source is gone.
#[derive(Clone)]
pub struct CustomVectorSourceHandle {
    loader: PoolActorRef<CustomVectorTileLoader>,
}

impl std::fmt::Debug for CustomVectorSourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSourceHandle").finish_non_exhaustive()
    }
}

impl CustomVectorSourceHandle {
    /// `setTileData(tileID, data, format)`: the tile's encoded bytes. `None`
    /// is a tile without data.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: Option<Data>, format: TileDataFormat) {
        self.loader.invoke(move |loader| loader.set_tile_data(&tile_id, (data, format)));
    }

    /// `setTileError(tileID, error)`: the host failed to load the tile.
    /// Invalidating the tile, or needing it again, asks for it again.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.loader.invoke(move |loader| loader.set_tile_error(&tile_id, &error));
    }

    /// `invalidateTile(tileID)`: drops the tile's data, and asks the host for
    /// it again if a tile needs it.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.loader.invoke(move |loader| loader.invalidate_tile(&tile_id));
    }

    /// `invalidateRegion(bounds)`: invalidates every tile that the bounds
    /// intersect.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.loader.invoke(move |loader| loader.invalidate_region(&bounds));
    }
}

/// `CustomVectorSource::Impl`.
#[derive(Clone)]
pub struct Impl {
    base: ImplBase,
    zoom_range: Range<u8>,
    loader_ref: Option<PoolActorRef<CustomVectorTileLoader>>,
}

impl std::fmt::Debug for Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSource::Impl")
            .field("base", &self.base)
            .field("zoom_range", &self.zoom_range)
            .field("loaded", &self.loader_ref.is_some())
            .finish()
    }
}

impl Impl {
    /// `Impl(std::string id, const CustomVectorSource::Options&)`.
    fn new(id: String, options: &Options) -> Self {
        Impl { base: ImplBase::new(SourceType::CustomMVTVector, id), zoom_range: options.zoom_range, loader_ref: None }
    }

    /// `Impl(const Impl&, const ActorRef<CustomVectorTileLoader>&)`.
    fn with_loader(other: &Impl, loader_ref: PoolActorRef<CustomVectorTileLoader>) -> Self {
        Impl { loader_ref: Some(loader_ref), ..other.clone() }
    }

    /// `operator!=`: whether tiles made for one impl don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Impl) -> bool {
        self.zoom_range != other.zoom_range || self.loader_ref.is_some() != other.loader_ref.is_some()
    }

    #[must_use]
    pub fn get_zoom_range(&self) -> Range<u8> {
        self.zoom_range
    }

    /// `getTileLoader()`: `None` until the source is loaded.
    #[must_use]
    pub fn get_tile_loader(&self) -> Option<&PoolActorRef<CustomVectorTileLoader>> {
        self.loader_ref.as_ref()
    }
}

impl source_impl::Impl for Impl {
    fn base(&self) -> &ImplBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ImplBase {
        &mut self.base
    }

    fn get_attribution(&self) -> Option<String> {
        None
    }
}

struct Inner {
    base: SourceBase,
    loader: Actor<CustomVectorTileLoader>,
}

/// `style::CustomVectorSource`: a source whose tiles' encoded vector data
/// the host gives on demand.
#[derive(Clone)]
pub struct CustomVectorSource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for CustomVectorSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSource").field("base", &self.inner.base).finish_non_exhaustive()
    }
}

impl CustomVectorSource {
    /// `CustomVectorSource(std::string id, const Options&)`.
    pub fn new(id: impl Into<String>, options: Options) -> Self {
        let impl_: Immutable<dyn source_impl::Impl> = Arc::new(Impl::new(id.into(), &options));
        let loader = Actor::new(scheduler::get_background(), |_| {
            CustomTileLoader::new(options.fetch_tile_function, options.cancel_tile_function)
        });
        CustomVectorSource { inner: Rc::new(Inner { base: SourceBase::new(impl_), loader }) }
    }

    /// `impl()`.
    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        static_immutable_cast::<Impl>(&self.inner.base.base_impl())
    }

    /// A handle to answer the tile functions with, from any thread.
    #[must_use]
    pub fn handle(&self) -> CustomVectorSourceHandle {
        CustomVectorSourceHandle { loader: self.inner.loader.self_ref() }
    }

    /// `setTileData(tileID, data, format)`; see [`CustomVectorSourceHandle`].
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: Option<Data>, format: TileDataFormat) {
        self.handle().set_tile_data(tile_id, data, format);
    }

    /// `setTileError(tileID, error)`; see [`CustomVectorSourceHandle`].
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.handle().set_tile_error(tile_id, error);
    }

    /// `invalidateTile(tileID)`; see [`CustomVectorSourceHandle`].
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.handle().invalidate_tile(tile_id);
    }

    /// `invalidateRegion(bounds)`; see [`CustomVectorSourceHandle`].
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.handle().invalidate_region(bounds);
    }
}

impl Source for CustomVectorSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, _file_source: &dyn FileSource) {
        let new_impl: Immutable<dyn source_impl::Impl> =
            Arc::new(Impl::with_loader(&self.impl_(), self.inner.loader.self_ref()));
        self.inner.base.set_base_impl(new_impl);
        self.inner.base.set_loaded(true);
        self.inner.base.observer().on_source_loaded(self);
    }

    fn supports_layer_type(&self, info: &LayerTypeInfo) -> bool {
        TileKind::Geometry as u8 == info.tile_kind as u8
    }

    fn create_mutable(&self) -> Box<dyn source_impl::Impl> {
        Box::new((*self.impl_()).clone())
    }
}
