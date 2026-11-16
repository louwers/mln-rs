//! Custom geometry and custom vector sources: tiles whose data the host
//! gives on demand, through tile functions that run on a background thread.

use std::sync::Arc;

use mln_actor::actor::{Actor, PoolActorRef};
use mln_storage::Data;
use mln_style_types::SourceType;
use mln_util::constants;
use mln_util::range::Range;

use super::geojson::TileFeatures;
use crate::custom_tile_loader::CustomTileLoader;
use crate::layer_type::SourceKind;
use crate::source_type::{SourceData, SourceTypeInfo, StyleSourceType};

/// How the host's GeoJSON is cut into tiles, as for a GeoJSON source.
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

/// The encoding of a custom vector tile's bytes.
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

/// The loader of a custom geometry source: it hands tiles their features,
/// cut from the host's GeoJSON.
pub type CustomGeometryTileLoader = CustomTileLoader<Arc<TileFeatures>>;

/// The loader of a custom vector source.
pub type CustomVectorTileLoader = CustomTileLoader<CustomVectorTileData>;

/// The custom geometry source type, whose tiles' GeoJSON the host gives.
pub struct CustomGeometrySourceType;

/// A custom geometry source's options.
#[derive(Clone)]
pub struct CustomGeometrySourceOptions {
    pub tile_options: Arc<TileOptions>,
    pub zoom_range: Range<u8>,
    /// The tile loader, which lives as long as a version of the source.
    pub loader: Arc<Actor<CustomGeometryTileLoader>>,
}

impl std::fmt::Debug for CustomGeometrySourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomGeometrySourceOptions")
            .field("tile_options", &self.tile_options)
            .field("zoom_range", &self.zoom_range)
            .finish_non_exhaustive()
    }
}

impl StyleSourceType for CustomGeometrySourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "custom-geometry",
        layer_kind: SourceKind::Geometry,
        from_json: false,
        type_: SourceType::CustomVector,
    };

    type Options = CustomGeometrySourceOptions;
    /// A reference to the options' tile loader.
    type Description = PoolActorRef<CustomGeometryTileLoader>;
}

impl SourceData<CustomGeometrySourceType> {
    /// Whether tiles made for one source don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Self) -> bool {
        !Arc::ptr_eq(&self.options.tile_options, &other.options.tile_options)
            || self.options.zoom_range != other.options.zoom_range
            || self.description.is_some() != other.description.is_some()
    }
}

/// The custom vector source type, whose tiles' encoded bytes the host gives.
pub struct CustomVectorSourceType;

/// A custom vector source's options.
#[derive(Clone)]
pub struct CustomVectorSourceOptions {
    pub zoom_range: Range<u8>,
    /// The tile loader, which lives as long as a version of the source.
    pub loader: Arc<Actor<CustomVectorTileLoader>>,
}

impl std::fmt::Debug for CustomVectorSourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSourceOptions").field("zoom_range", &self.zoom_range).finish_non_exhaustive()
    }
}

impl StyleSourceType for CustomVectorSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "custom-vector",
        layer_kind: SourceKind::Geometry,
        from_json: false,
        type_: SourceType::CustomMVTVector,
    };

    type Options = CustomVectorSourceOptions;
    /// A reference to the options' tile loader.
    type Description = PoolActorRef<CustomVectorTileLoader>;
}

impl SourceData<CustomVectorSourceType> {
    /// Whether tiles made for one source don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Self) -> bool {
        self.options.zoom_range != other.options.zoom_range || self.description.is_some() != other.description.is_some()
    }
}
