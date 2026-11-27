//! Custom geometry sources, whose tiles' GeoJSON the host gives on demand,
//! which each tile cuts as a GeoJSON source's would.

use std::sync::Arc;

use geojson::GeoJson;
use mln_builtins::sources::geojson::TileFeatures;
use mln_style::constants;
use mln_style::layer::SourceKind;
use mln_style::range::Range;
use mln_style::source::{SourceTypeInfo, StyleSourceType};
use mln_style::tile_id::CanonicalTileID;

use super::TileFunctions;
use crate::log_target;

/// How the host's GeoJSON is cut into tiles, as for a GeoJSON source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TileOptions {
    pub(crate) tolerance: f64,
    /// The size of a tile, in pixels: a power of two up to the tile extent
    /// (8192).
    pub(crate) tile_size: u16,
    pub(crate) buffer: u16,
    pub(crate) clip: bool,
    pub(crate) wrap: bool,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tolerance: 0.375, tile_size: constants::TILE_SIZE_I, buffer: 128, clip: false, wrap: false }
    }
}

/// The custom geometry source type, whose tiles' GeoJSON the host gives.
pub(crate) struct CustomGeometrySourceType;

/// A custom geometry source's options.
#[derive(Clone)]
pub(crate) struct CustomGeometrySourceOptions {
    pub(crate) tile_options: Arc<TileOptions>,
    pub(crate) zoom_range: Range<u8>,
    /// The host's functions that give the tiles' GeoJSON.
    pub(crate) tile_functions: Arc<TileFunctions<GeoJson>>,
}

impl std::fmt::Debug for CustomGeometrySourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceOptions")
            .field("tile_options", &self.tile_options)
            .field("zoom_range", &self.zoom_range)
            .field("tile_functions", &self.tile_functions)
            .finish()
    }
}

impl StyleSourceType for CustomGeometrySourceType {
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "custom-geometry", layer_kind: SourceKind::Geometry, from_json: false };

    type Options = CustomGeometrySourceOptions;
}

impl CustomGeometrySourceOptions {
    /// Whether tiles made for one source don't suit the other.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        !Arc::ptr_eq(&self.tile_options, &other.tile_options)
            || self.zoom_range != other.zoom_range
            || !Arc::ptr_eq(&self.tile_functions, &other.tile_functions)
    }
}

/// Cuts the host's GeoJSON for tile `tile_id` with a source's `options`.
/// Anything but a non-empty feature collection is an empty tile, and so is
/// one whose geometry `geo_types` can't hold, which is logged.
pub(crate) fn tile_features(geojson: GeoJson, tile_id: &CanonicalTileID, options: &TileOptions) -> TileFeatures {
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
    let scale = f64::from(constants::EXTENT / i32::from(options.tile_size.max(1)));
    let tile_options = mln_geojson::tile::TileOptions {
        extent: constants::EXTENT as u16,
        buffer: (scale * f64::from(options.buffer)).round() as u16,
        tolerance: scale * options.tolerance,
        ..Default::default()
    };
    mln_geojson::tile::cut_tile(&features, tile_id.z, tile_id.x, tile_id.y, &tile_options, options.wrap, options.clip)
}
