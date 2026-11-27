//! Custom vector sources, whose tiles' encoded bytes the host gives on
//! demand.

use std::sync::Arc;

use mln_engine::storage::Data;
use mln_style::layer::SourceKind;
use mln_style::range::Range;
use mln_style::source::{SourceTypeInfo, StyleSourceType};

use super::TileFunctions;

/// The encoding of a custom vector tile's bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TileDataFormat {
    /// Mapbox Vector Tile.
    #[default]
    Mvt,
    /// MapLibre Tile, decoded with the map's FastPFOR option.
    Mlt,
}

/// The data a custom vector tile gets: its bytes, if any, and their format.
pub(crate) type CustomVectorTileData = (Option<Data>, TileDataFormat);

/// The custom vector source type, whose tiles' encoded bytes the host gives.
pub(crate) struct CustomVectorSourceType;

/// A custom vector source's options.
#[derive(Clone)]
pub(crate) struct CustomVectorSourceOptions {
    pub(crate) zoom_range: Range<u8>,
    /// The host's functions that give the tiles' bytes.
    pub(crate) tile_functions: Arc<TileFunctions<CustomVectorTileData>>,
}

impl std::fmt::Debug for CustomVectorSourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSourceOptions")
            .field("zoom_range", &self.zoom_range)
            .field("tile_functions", &self.tile_functions)
            .finish()
    }
}

impl StyleSourceType for CustomVectorSourceType {
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "custom-vector",
        layer_kind: SourceKind::Geometry,
        from_json: false,
        round_zoom: false,
        overscale: true,
        prefetch: true,
    };

    type Options = CustomVectorSourceOptions;
    /// None: the host's tile functions give the source's data.
    type Description = ();
}

impl CustomVectorSourceOptions {
    /// Whether tiles made for one source don't suit the other.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        self.zoom_range != other.zoom_range || !Arc::ptr_eq(&self.tile_functions, &other.tile_functions)
    }
}
