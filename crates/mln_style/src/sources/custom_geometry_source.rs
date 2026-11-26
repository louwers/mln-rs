//! Custom geometry sources, whose tiles' GeoJSON the host gives on demand.
//! The host answers its tile functions, which run on a pool thread, through
//! the [`CustomGeometrySourceHandle`] they get.

use std::sync::Arc;

use geojson::GeoJson;

pub use super::custom::{CustomGeometrySourceHandle, TileOptions};
use super::custom::{CustomGeometrySourceOptions, CustomGeometrySourceType, TileFunction, TileFunctions};
use super::geojson::TileFeatures;
use crate::range::Range;
use crate::source::{Source, SourceData};
use crate::tile_id::CanonicalTileID;
use crate::{constants, log_target};

#[derive(Clone)]
pub struct Options {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomGeometrySourceHandle::set_tile_data`].
    pub fetch_tile_function: Option<TileFunction<GeoJson>>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile_function: Option<TileFunction<GeoJson>>,
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

/// Cuts the host's GeoJSON for tile `tile_id` with a source's `options`, as
/// `CustomGeometryTile::setTileData` does. Anything but a non-empty feature
/// collection is an empty tile, and so is one whose geometry `geo_types`
/// can't hold, which is logged.
#[must_use]
pub fn tile_features(geojson: GeoJson, tile_id: &CanonicalTileID, options: &TileOptions) -> TileFeatures {
    if !matches!(&geojson, GeoJson::FeatureCollection(c) if !c.features.is_empty()) {
        return TileFeatures::new();
    }
    let features = match feature::geojson::features(geojson) {
        Ok(features) => features,
        Err(e) => {
            log::error!(target: log_target::GENERAL, "Invalid custom geometry tile data: {e}");
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

/// The custom geometry source `id`. Fails for a tile size that isn't a power
/// of two up to the tile extent.
pub fn create(id: &str, options: Options) -> Result<Source, CustomGeometrySourceError> {
    let tile_size = options.tile_options.tile_size;
    if !tile_size.is_power_of_two() || i32::from(tile_size) > constants::EXTENT {
        return Err(CustomGeometrySourceError::InvalidTileSize(tile_size));
    }
    let tile_functions =
        TileFunctions { fetch_tile: options.fetch_tile_function, cancel_tile: options.cancel_tile_function };
    let options = CustomGeometrySourceOptions {
        tile_options: Arc::new(options.tile_options),
        zoom_range: options.zoom_range,
        tile_functions: Arc::new(tile_functions),
    };
    Ok(Source::new(id, SourceData::<CustomGeometrySourceType>::new(options)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_tile_size(tile_size: u16) -> Result<Source, CustomGeometrySourceError> {
        let mut options = Options::default();
        options.tile_options.tile_size = tile_size;
        create("custom", options)
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
