// `CustomVectorSource` is a handle (see source.rs): `Clone` shares the
// source. As for a custom geometry source, the host answers its tile
// functions through a `Send` [`CustomVectorSourceHandle`].

use std::rc::Rc;

use mln_actor::actor::{Actor, PoolActorRef};
use mln_storage::{Data, FileSource};
use mln_tile_id::CanonicalTileID;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::range::Range;

use super::custom::{CustomVectorSourceOptions, CustomVectorSourceType};
pub use super::custom::{CustomVectorTileData, CustomVectorTileLoader, TileDataFormat};
use crate::custom_tile_loader::{CustomTileLoader, TileFunction};
use crate::source::{Source, SourceBase};
use crate::source_type::{self, SourceData};

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
    /// Hands over the tile's encoded bytes. `None` is a tile without data.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: Option<Data>, format: TileDataFormat) {
        self.loader.invoke(move |loader| loader.set_tile_data(&tile_id, (data, format)));
    }

    /// Reports that the host failed to load the tile. Invalidating the tile, or
    /// needing it again, asks for it again.
    pub(crate) fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.loader.invoke(move |loader| loader.set_tile_error(&tile_id, &error));
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub(crate) fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.loader.invoke(move |loader| loader.invalidate_tile(&tile_id));
    }

    /// Invalidates every tile that `bounds` intersect.
    pub(crate) fn invalidate_region(&self, bounds: LatLngBounds) {
        self.loader.invoke(move |loader| loader.invalidate_region(&bounds));
    }
}

struct Inner {
    base: SourceBase,
    loader: Actor<CustomVectorTileLoader>,
}

/// A source whose tiles' encoded vector data the host gives on demand.
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
    /// Creates the source, whose tile functions run on `pool`.
    pub fn new(id: impl Into<String>, options: Options, pool: mln_actor::SchedulerPtr) -> Self {
        let data =
            SourceData::<CustomVectorSourceType>::new(CustomVectorSourceOptions { zoom_range: options.zoom_range });
        let record = source_type::Source::new(&id.into(), data);
        let loader =
            Actor::new(pool, |_| CustomTileLoader::new(options.fetch_tile_function, options.cancel_tile_function));
        CustomVectorSource { inner: Rc::new(Inner { base: SourceBase::new(record), loader }) }
    }

    /// Records the source's data as `f` changes it.
    fn update(&self, f: impl FnOnce(&mut SourceData<CustomVectorSourceType>)) {
        if let Some(record) = self.inner.base.base_impl().map_data(f) {
            self.inner.base.set_base_impl(record);
        }
    }

    /// A handle to answer the tile functions with, from any thread.
    #[must_use]
    pub fn handle(&self) -> CustomVectorSourceHandle {
        CustomVectorSourceHandle { loader: self.inner.loader.self_ref() }
    }

    /// See [`CustomVectorSourceHandle::set_tile_data`].
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: Option<Data>, format: TileDataFormat) {
        self.handle().set_tile_data(tile_id, data, format);
    }

    /// See `CustomVectorSourceHandle::set_tile_error`.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.handle().set_tile_error(tile_id, error);
    }

    /// See `CustomVectorSourceHandle::invalidate_tile`.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.handle().invalidate_tile(tile_id);
    }

    /// See `CustomVectorSourceHandle::invalidate_region`.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.handle().invalidate_region(bounds);
    }
}

impl Source for CustomVectorSource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, _file_source: &dyn FileSource, _run_loop: &mln_actor::RunLoop) {
        self.update(|data| data.description = Some(self.inner.loader.self_ref()));
        self.inner.base.set_loaded(true);
        self.inner.base.observer().on_source_loaded(self);
    }
}
