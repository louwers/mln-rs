//! Custom geometry sources, whose tiles' GeoJSON the host gives on demand.
//! The host answers its tile functions, which run on a pool thread, through
//! a [`CustomGeometrySourceHandle`].

use std::sync::{Arc, Weak};

use geojson::GeoJson;
use mln_style::source_type::{Source, SourceData};
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::error::Error;
use mln_util::geo::LatLngBounds;
use mln_util::logging::{Event, Log};
use mln_util::pool::Pool;
use mln_util::range::Range;

use super::provider::{CustomProvider, TileFunction};
use super::{CustomGeometryProvider, CustomGeometrySourceOptions, CustomGeometrySourceType, TileOptions};
use crate::geojson::TileFeatures;

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

/// Why a custom geometry source can't be made.
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
    // `create` checked that the tile size divides the extent.
    let scale = f64::from(constants::EXTENT / i32::from(options.tile_size.max(1)));
    let tile_options = mln_geojson::tile::TileOptions {
        extent: constants::EXTENT as u16,
        buffer: (scale * f64::from(options.buffer)).round() as u16,
        tolerance: scale * options.tolerance,
        ..Default::default()
    };
    mln_geojson::tile::cut_tile(&features, tile_id.z, tile_id.x, tile_id.y, &tile_options, options.wrap, options.clip)
}

/// A `Send` handle to a custom geometry source's provider, for the host to
/// answer its tile functions from any thread. It does nothing once the
/// source is gone.
#[derive(Clone)]
pub struct CustomGeometrySourceHandle {
    provider: Weak<CustomGeometryProvider>,
    tile_options: Arc<TileOptions>,
}

impl std::fmt::Debug for CustomGeometrySourceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceHandle").finish_non_exhaustive()
    }
}

impl CustomGeometrySourceHandle {
    /// The handle of `source`, if it's a custom geometry source.
    #[must_use]
    pub fn of(source: &Source) -> Option<Self> {
        let data = SourceData::<CustomGeometrySourceType>::recover(source)?;
        Some(CustomGeometrySourceHandle {
            provider: Arc::downgrade(&data.options.provider),
            tile_options: Arc::clone(&data.options.tile_options),
        })
    }

    /// Hands over the tile's features. The GeoJSON is cut to the tile on the
    /// provider's pool thread, and only if a tile needs it.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: GeoJson) {
        if let Some(provider) = self.provider.upgrade() {
            let options = Arc::clone(&self.tile_options);
            provider.set_tile_data_with(tile_id, move || Arc::new(tile_features(data, &tile_id, &options)));
        }
    }

    /// Reports that the host failed to load the tile. Invalidating the tile, or
    /// needing it again, asks for it again.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        if let Some(provider) = self.provider.upgrade() {
            provider.set_tile_error(tile_id, error);
        }
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_tile(tile_id);
        }
    }

    /// Invalidates every tile that `bounds` intersect.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        if let Some(provider) = self.provider.upgrade() {
            provider.invalidate_region(bounds);
        }
    }
}

/// The custom geometry source `id`, whose tile functions run on `pool`.
/// Fails for a tile size that isn't a power of two up to the tile extent.
pub fn create(id: &str, options: Options, pool: &Pool) -> Result<Source, CustomGeometrySourceError> {
    let tile_size = options.tile_options.tile_size;
    if !tile_size.is_power_of_two() || i32::from(tile_size) > constants::EXTENT {
        return Err(CustomGeometrySourceError::InvalidTileSize(tile_size));
    }
    let provider = CustomProvider::new(options.fetch_tile_function, options.cancel_tile_function, pool);
    let options = CustomGeometrySourceOptions {
        tile_options: Arc::new(options.tile_options),
        zoom_range: options.zoom_range,
        provider,
    };
    Ok(Source::new(id, SourceData::<CustomGeometrySourceType>::new(options)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tile_size(tile_size: u16) -> Result<Source, CustomGeometrySourceError> {
        let mut options = Options::default();
        options.tile_options.tile_size = tile_size;
        create("custom", options, &Pool::local())
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
