// Ports include/mln/style/sources/custom_geometry_source.hpp,
// src/mln/style/sources/custom_geometry_source.cpp and
// src/mln/style/sources/custom_geometry_source_impl.{hpp,cpp}.
//
// `CustomGeometrySource` is a handle (see source.rs): `Clone` shares the
// source. A source isn't `Send`, while the host's tile functions run on a
// background thread, so the host answers through a
// [`CustomGeometrySourceHandle`], which is.

use std::rc::Rc;
use std::sync::Arc;

use geojson::GeoJSON;
use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::scheduler;
use mln_storage::FileSource;
use mln_style_types::SourceType;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::immutable::Immutable;
use mln_util::range::Range;

use super::geojson_source::TileFeatures;
use crate::custom_tile_loader::{CustomTileLoader, TileFunction};
use crate::layer::LayerTypeInfo;
use crate::layer::layer_type_info::TileKind;
use crate::source::{Source, SourceBase, static_immutable_cast};
use crate::source_impl::{self, ImplBase};

/// `CustomGeometrySource::TileOptions`: how the host's GeoJSON is cut into
/// tiles, as for a GeoJSON source.
#[derive(Clone, Debug, PartialEq)]
pub struct TileOptions {
    pub tolerance: f64,
    /// The size of a tile, in pixels: a power of two up to the tile extent
    /// (8192).
    pub tile_size: u16,
    pub buffer: u16,
    pub clip: bool,
    pub wrap: bool,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tolerance: 0.375, tile_size: constants::TILE_SIZE_I, buffer: 128, clip: false, wrap: false }
    }
}

/// `CustomGeometrySource::Options`.
#[derive(Clone)]
pub struct Options {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomGeometrySourceHandle::set_tile_data`].
    pub fetch_tile_function: Option<TileFunction>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile_function: Option<TileFunction>,
    pub zoom_range: Range<u8>,
    pub tile_options: TileOptions,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fetch_tile_function: None,
            cancel_tile_function: None,
            zoom_range: Range::new(0, 18),
            tile_options: TileOptions::default(),
        }
    }
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("fetch_tile_function", &self.fetch_tile_function.is_some())
            .field("cancel_tile_function", &self.cancel_tile_function.is_some())
            .field("zoom_range", &self.zoom_range)
            .field("tile_options", &self.tile_options)
            .finish()
    }
}

/// Why a [`CustomGeometrySource`] can't be made.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CustomGeometrySourceError {
    /// The tile size isn't a power of two up to the tile extent, so tiles
    /// can't be scaled to the extent.
    #[error("custom geometry source tile size {0} is not a power of two up to {extent}", extent = constants::EXTENT)]
    InvalidTileSize(u16),
}

/// The loader of a custom geometry source: it hands tiles their features,
/// cut from the host's GeoJSON.
pub type CustomGeometryTileLoader = CustomTileLoader<Arc<TileFeatures>>;

/// Cuts the host's GeoJSON for a tile, as `CustomGeometryTile::setTileData`
/// does. Anything but a non-empty feature collection is an empty tile.
fn tile_features(geojson: &GeoJSON, tile_id: &CanonicalTileID, options: &TileOptions) -> TileFeatures {
    let GeoJSON::FeatureCollection(features) = geojson else {
        return TileFeatures::new();
    };
    if features.is_empty() {
        return TileFeatures::new();
    }
    // `CustomGeometrySource::new` checked that the tile size divides the
    // extent.
    let scale = f64::from(constants::EXTENT / i32::from(options.tile_size.max(1)));
    let vt_options = geojson_vt::TileOptions {
        extent: constants::EXTENT as u16,
        buffer: (scale * f64::from(options.buffer)).round() as u16,
        tolerance: scale * options.tolerance,
        ..Default::default()
    };
    geojson_vt::geojson_to_tile(geojson, tile_id.z, tile_id.x, tile_id.y, &vt_options, options.wrap, options.clip)
        .features
}

/// A `Send` handle to a custom geometry source's loader, for the host to
/// answer its tile functions from any thread. It does nothing once the
/// source is gone.
#[derive(Clone)]
pub struct CustomGeometrySourceHandle {
    loader: PoolActorRef<CustomGeometryTileLoader>,
    tile_options: Immutable<TileOptions>,
}

impl std::fmt::Debug for CustomGeometrySourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceHandle").finish_non_exhaustive()
    }
}

impl CustomGeometrySourceHandle {
    /// `setTileData(tileID, data)`: the tile's features. The GeoJSON is cut
    /// to the tile on the background thread, and only if a tile needs it.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: GeoJSON) {
        let options = self.tile_options.clone();
        self.loader.invoke(move |loader| {
            if loader.is_required(&tile_id) {
                let features = tile_features(&data, &tile_id, &options);
                loader.set_tile_data(&tile_id, Arc::new(features));
            }
        });
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

/// `CustomGeometrySource::Impl`.
#[derive(Clone)]
pub struct Impl {
    base: ImplBase,
    tile_options: Immutable<TileOptions>,
    zoom_range: Range<u8>,
    loader_ref: Option<PoolActorRef<CustomGeometryTileLoader>>,
}

impl std::fmt::Debug for Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySource::Impl")
            .field("base", &self.base)
            .field("tile_options", &self.tile_options)
            .field("zoom_range", &self.zoom_range)
            .field("loaded", &self.loader_ref.is_some())
            .finish()
    }
}

impl Impl {
    /// `Impl(std::string id, const CustomGeometrySource::Options&)`.
    fn new(id: String, options: &Options) -> Self {
        Impl {
            base: ImplBase::new(SourceType::CustomVector, id),
            tile_options: Arc::new(options.tile_options.clone()),
            zoom_range: options.zoom_range,
            loader_ref: None,
        }
    }

    /// `Impl(const Impl&, const ActorRef<CustomTileLoader>&)`.
    fn with_loader(other: &Impl, loader_ref: PoolActorRef<CustomGeometryTileLoader>) -> Self {
        Impl { loader_ref: Some(loader_ref), ..other.clone() }
    }

    /// `operator!=`: whether tiles made for one impl don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Impl) -> bool {
        !Arc::ptr_eq(&self.tile_options, &other.tile_options)
            || self.zoom_range != other.zoom_range
            || self.loader_ref.is_some() != other.loader_ref.is_some()
    }

    #[must_use]
    pub fn get_tile_options(&self) -> &Immutable<TileOptions> {
        &self.tile_options
    }

    #[must_use]
    pub fn get_zoom_range(&self) -> Range<u8> {
        self.zoom_range
    }

    /// `getTileLoader()`: `None` until the source is loaded.
    #[must_use]
    pub fn get_tile_loader(&self) -> Option<&PoolActorRef<CustomGeometryTileLoader>> {
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
    loader: Actor<CustomGeometryTileLoader>,
}

/// `style::CustomGeometrySource`: a source whose tiles' GeoJSON the host
/// gives on demand.
#[derive(Clone)]
pub struct CustomGeometrySource {
    inner: Rc<Inner>,
}

impl std::fmt::Debug for CustomGeometrySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySource").field("base", &self.inner.base).finish_non_exhaustive()
    }
}

impl CustomGeometrySource {
    /// `CustomGeometrySource(std::string id, const Options&)`. `Err` for a
    /// tile size that isn't a power of two up to the tile extent.
    pub fn new(id: impl Into<String>, options: Options) -> Result<Self, CustomGeometrySourceError> {
        let tile_size = options.tile_options.tile_size;
        if !tile_size.is_power_of_two() || i32::from(tile_size) > constants::EXTENT {
            return Err(CustomGeometrySourceError::InvalidTileSize(tile_size));
        }
        let impl_: Immutable<dyn source_impl::Impl> = Arc::new(Impl::new(id.into(), &options));
        let loader = Actor::new(scheduler::get_background(), |_| {
            CustomTileLoader::new(options.fetch_tile_function, options.cancel_tile_function)
        });
        Ok(CustomGeometrySource { inner: Rc::new(Inner { base: SourceBase::new(impl_), loader }) })
    }

    /// `impl()`.
    #[must_use]
    pub fn impl_(&self) -> Arc<Impl> {
        static_immutable_cast::<Impl>(&self.inner.base.base_impl())
    }

    /// A handle to answer the tile functions with, from any thread.
    #[must_use]
    pub fn handle(&self) -> CustomGeometrySourceHandle {
        CustomGeometrySourceHandle {
            loader: self.inner.loader.self_ref(),
            tile_options: self.impl_().tile_options.clone(),
        }
    }

    /// `setTileData(tileID, data)`; see [`CustomGeometrySourceHandle`].
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: GeoJSON) {
        self.handle().set_tile_data(tile_id, data);
    }

    /// `setTileError(tileID, error)`; see [`CustomGeometrySourceHandle`].
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.handle().set_tile_error(tile_id, error);
    }

    /// `invalidateTile(tileID)`; see [`CustomGeometrySourceHandle`].
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.handle().invalidate_tile(tile_id);
    }

    /// `invalidateRegion(bounds)`; see [`CustomGeometrySourceHandle`].
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.handle().invalidate_region(bounds);
    }
}

impl Source for CustomGeometrySource {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tile_size(tile_size: u16) -> Result<CustomGeometrySource, CustomGeometrySourceError> {
        let mut options = Options::default();
        options.tile_options.tile_size = tile_size;
        CustomGeometrySource::new("custom", options)
    }

    #[test]
    fn tile_size_must_divide_the_extent() {
        for tile_size in [0, 3, 500, 16384] {
            assert_eq!(with_tile_size(tile_size).err(), Some(CustomGeometrySourceError::InvalidTileSize(tile_size)));
        }
        for tile_size in [1, 16, 512, 8192] {
            assert!(with_tile_size(tile_size).is_ok());
        }
    }
}
