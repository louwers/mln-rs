// `CustomGeometrySource` is a handle (see source.rs): `Clone` shares the
// source. A source isn't `Send`, while the host's tile functions run on a
// background thread, so the host answers through a
// [`CustomGeometrySourceHandle`], which is.

use std::rc::Rc;
use std::sync::Arc;

use geojson::GeoJson;
use mln_actor::actor::{Actor, PoolActorRef};
use mln_storage::FileSource;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::logging::{Event, Log};
use mln_util::range::Range;

use super::custom::{CustomGeometrySourceOptions, CustomGeometrySourceType};
pub use super::custom::{CustomGeometryTileLoader, TileOptions};
use super::geojson::TileFeatures;
use crate::custom_tile_loader::{CustomTileLoader, TileFunction};
use crate::source::{Source, SourceBase};
use crate::source_type::{self, SourceData};

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

/// Cuts the host's GeoJSON for a tile, as `CustomGeometryTile::setTileData`
/// does. Anything but a non-empty feature collection is an empty tile, and
/// so is one whose geometry `geo_types` can't hold, which is logged.
fn tile_features(geojson: GeoJson, tile_id: &CanonicalTileID, options: &TileOptions) -> TileFeatures {
    if !matches!(&geojson, GeoJson::FeatureCollection(c) if !c.features.is_empty()) {
        return TileFeatures::new();
    }
    let features = match feature::geojson::features(geojson) {
        Ok(features) => features,
        Err(e) => {
            Log::error(Event::General, format!("Invalid custom geometry tile data: {e}"));
            return TileFeatures::new();
        }
    };
    // `CustomGeometrySource::new` checked that the tile size divides the
    // extent.
    let scale = f64::from(constants::EXTENT / i32::from(options.tile_size.max(1)));
    let vt_options = geojson_vt::TileOptions {
        extent: constants::EXTENT as u16,
        buffer: (scale * f64::from(options.buffer)).round() as u16,
        tolerance: scale * options.tolerance,
        ..Default::default()
    };
    geojson_vt::geojson_to_tile(&features, tile_id.z, tile_id.x, tile_id.y, &vt_options, options.wrap, options.clip)
        .features
}

/// A `Send` handle to a custom geometry source's loader, for the host to
/// answer its tile functions from any thread. It does nothing once the
/// source is gone.
#[derive(Clone)]
pub struct CustomGeometrySourceHandle {
    loader: PoolActorRef<CustomGeometryTileLoader>,
    tile_options: Arc<TileOptions>,
}

impl std::fmt::Debug for CustomGeometrySourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceHandle").finish_non_exhaustive()
    }
}

impl CustomGeometrySourceHandle {
    /// Hands over the tile's features. The GeoJSON is cut to the tile on the
    /// background thread, and only if a tile needs it.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: GeoJson) {
        let options = Arc::clone(&self.tile_options);
        self.loader.invoke(move |loader| {
            if loader.is_required(&tile_id) {
                let features = tile_features(data, &tile_id, &options);
                loader.set_tile_data(&tile_id, Arc::new(features));
            }
        });
    }

    /// Reports that the host failed to load the tile. Invalidating the tile, or
    /// needing it again, asks for it again.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
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
    loader: Actor<CustomGeometryTileLoader>,
    tile_options: Arc<TileOptions>,
}

/// A source whose tiles' GeoJSON the host gives on demand.
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
    /// Creates the source, whose tile functions run on `pool`. Fails for a
    /// tile size that isn't a power of two up to the tile extent.
    pub fn new(
        id: impl Into<String>,
        options: Options,
        pool: mln_actor::SchedulerPtr,
    ) -> Result<Self, CustomGeometrySourceError> {
        let tile_size = options.tile_options.tile_size;
        if !tile_size.is_power_of_two() || i32::from(tile_size) > constants::EXTENT {
            return Err(CustomGeometrySourceError::InvalidTileSize(tile_size));
        }
        let tile_options = Arc::new(options.tile_options.clone());
        let data = SourceData::<CustomGeometrySourceType>::new(CustomGeometrySourceOptions {
            tile_options: Arc::clone(&tile_options),
            zoom_range: options.zoom_range,
        });
        let record = source_type::Source::new(&id.into(), data);
        let loader =
            Actor::new(pool, |_| CustomTileLoader::new(options.fetch_tile_function, options.cancel_tile_function));
        Ok(CustomGeometrySource { inner: Rc::new(Inner { base: SourceBase::new(record), loader, tile_options }) })
    }

    /// Records the source's data as `f` changes it.
    fn update(&self, f: impl FnOnce(&mut SourceData<CustomGeometrySourceType>)) {
        if let Some(record) = self.inner.base.base_impl().map_data(f) {
            self.inner.base.set_base_impl(record);
        }
    }

    /// A handle to answer the tile functions with, from any thread.
    #[must_use]
    pub fn handle(&self) -> CustomGeometrySourceHandle {
        CustomGeometrySourceHandle {
            loader: self.inner.loader.self_ref(),
            tile_options: Arc::clone(&self.inner.tile_options),
        }
    }

    /// See [`CustomGeometrySourceHandle::set_tile_data`].
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: GeoJson) {
        self.handle().set_tile_data(tile_id, data);
    }

    /// See [`CustomGeometrySourceHandle::set_tile_error`].
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.handle().set_tile_error(tile_id, error);
    }

    /// See `CustomGeometrySourceHandle::invalidate_tile`.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.handle().invalidate_tile(tile_id);
    }

    /// See `CustomGeometrySourceHandle::invalidate_region`.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.handle().invalidate_region(bounds);
    }
}

impl Source for CustomGeometrySource {
    fn base(&self) -> &SourceBase {
        &self.inner.base
    }

    fn load_description(&self, _file_source: &dyn FileSource, _run_loop: &mln_actor::RunLoop) {
        self.update(|data| data.description = Some(self.inner.loader.self_ref()));
        self.inner.base.set_loaded(true);
        self.inner.base.observer().on_source_loaded(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tile_size(tile_size: u16) -> Result<CustomGeometrySource, CustomGeometrySourceError> {
        let mut options = Options::default();
        options.tile_options.tile_size = tile_size;
        CustomGeometrySource::new("custom", options, mln_actor::scheduler::get_background())
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
