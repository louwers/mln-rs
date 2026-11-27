//! Custom vector sources, whose tiles' encoded bytes the host gives on
//! demand.

use std::sync::Arc;

use mln_engine::storage::Data;
use mln_style::crs::{CrsMapping, same_mapping};
use mln_style::layer::SourceKind;
use mln_style::range::Range;
use mln_style::source::{SourceData, SourceTypeInfo, StyleSourceType};

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
    /// The registered projection that the source's style is drawn in, whose
    /// grid the tiles are in and whose world square the regions it
    /// invalidates lie in; `None` in the built-in projections. The style sets
    /// it when the source joins it and when its projection changes.
    pub(crate) crs: Option<Arc<CrsMapping>>,
}

impl std::fmt::Debug for CustomVectorSourceOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomVectorSourceOptions")
            .field("zoom_range", &self.zoom_range)
            .field("tile_functions", &self.tile_functions)
            .field("crs", &self.crs)
            .finish()
    }
}

impl StyleSourceType for CustomVectorSourceType {
    const INFO: SourceTypeInfo =
        SourceTypeInfo { name: "custom-vector", layer_kind: SourceKind::Geometry, from_json: false };

    type Options = CustomVectorSourceOptions;

    /// The source with `crs` in its options, whose grid its tiles are in.
    fn in_projection(source: &SourceData<Self>, crs: Option<&Arc<CrsMapping>>) -> Option<SourceData<Self>> {
        if same_mapping(source.options.crs.as_ref(), crs) {
            return None;
        }
        let mut source = source.clone();
        source.options.crs = crs.cloned();
        Some(source)
    }
}

impl CustomVectorSourceOptions {
    /// Whether tiles made for one source don't suit the other.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        self.zoom_range != other.zoom_range
            || !Arc::ptr_eq(&self.tile_functions, &other.tile_functions)
            || !same_mapping(self.crs.as_ref(), other.crs.as_ref())
    }
}
