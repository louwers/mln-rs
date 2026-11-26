//! Custom geometry and custom vector sources: tiles whose data the host
//! gives on demand. Both types' sources hold the host's tile functions,
//! which the engine calls on a pool thread with a [`CustomSourceHandle`] that
//! the host answers through.

use std::sync::Arc;

use geojson::GeoJson;

use crate::constants;
use crate::error::Error;
use crate::geo::LatLngBounds;
use crate::layer::SourceKind;
use crate::range::Range;
use crate::source::{Data, SourceData, SourceTypeInfo, StyleSourceType};
use crate::tile_id::CanonicalTileID;

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

/// Called with the canonical ID of a tile that the map needs, or no longer
/// needs, and the handle that the host answers through. It runs on a pool
/// thread, one call at a time, in the order the tiles asked, so a host may
/// answer from inside it: the answer takes effect after the call.
pub type TileFunction<D> = Arc<dyn Fn(&CanonicalTileID, &CustomSourceHandle<D>) + Send + Sync>;

/// A custom source's tile functions, which take answers of type `D`.
pub struct TileFunctions<D> {
    /// Asks the host for a tile's data, which it gives with
    /// [`CustomSourceHandle::set_tile_data`].
    pub fetch_tile: Option<TileFunction<D>>,
    /// Tells the host that no tile needs a tile's data any more.
    pub cancel_tile: Option<TileFunction<D>>,
}

impl<D> std::fmt::Debug for TileFunctions<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileFunctions")
            .field("fetch_tile", &self.fetch_tile.is_some())
            .field("cancel_tile", &self.cancel_tile.is_some())
            .finish()
    }
}

/// What a custom source's handle reaches: the engine's provider of the
/// source's tiles, which an engine implements.
pub trait CustomTileResponder<D>: Send + Sync {
    /// Hands over the tile's data. It is dropped unless a tile needs it.
    fn set_tile_data(&self, tile_id: CanonicalTileID, data: D);
    /// Reports that the host failed to load the tile.
    fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error);
    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    fn invalidate_tile(&self, tile_id: CanonicalTileID);
    /// Invalidates every tile that `bounds` intersect, at its own zoom.
    fn invalidate_region(&self, bounds: LatLngBounds);
    /// Whether the provider is gone, so that the calls above do nothing.
    fn is_closed(&self) -> bool;
}

/// A `Send` handle to the engine's provider of a custom source's tiles,
/// which the provider hands to the host's tile functions. The host answers
/// through it, from any thread; it does nothing once the provider is gone.
/// Clones reach the same provider.
pub struct CustomSourceHandle<D>(Arc<dyn CustomTileResponder<D>>);

/// The handle of a custom geometry source, which takes the tiles' GeoJSON.
pub type CustomGeometrySourceHandle = CustomSourceHandle<GeoJson>;

/// The handle of a custom vector source, which takes the tiles' encoded
/// bytes.
pub type CustomVectorSourceHandle = CustomSourceHandle<CustomVectorTileData>;

impl<D> Clone for CustomSourceHandle<D> {
    fn clone(&self) -> Self {
        CustomSourceHandle(Arc::clone(&self.0))
    }
}

impl<D> std::fmt::Debug for CustomSourceHandle<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomSourceHandle").finish_non_exhaustive()
    }
}

impl<D> CustomSourceHandle<D> {
    /// The handle of the provider behind `responder`.
    #[must_use]
    pub fn new(responder: Arc<dyn CustomTileResponder<D>>) -> Self {
        CustomSourceHandle(responder)
    }

    /// Hands over the tile's data. The source ignores data for a tile it
    /// hasn't asked for, or has since cancelled.
    pub fn set_tile_data(&self, tile_id: CanonicalTileID, data: D) {
        self.0.set_tile_data(tile_id, data);
    }

    /// Reports that the host failed to load the tile. Invalidating the tile,
    /// or needing it again, asks for it again.
    pub fn set_tile_error(&self, tile_id: CanonicalTileID, error: Error) {
        self.0.set_tile_error(tile_id, error);
    }

    /// Drops the tile's data, and asks the host for it again if a tile needs
    /// it.
    pub fn invalidate_tile(&self, tile_id: CanonicalTileID) {
        self.0.invalidate_tile(tile_id);
    }

    /// Invalidates every tile that `bounds` intersect.
    pub fn invalidate_region(&self, bounds: LatLngBounds) {
        self.0.invalidate_region(bounds);
    }

    /// Whether the provider is gone, so that the handle does nothing.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Whether both handles reach the same provider.
    #[must_use]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// The custom geometry source type, whose tiles' GeoJSON the host gives.
pub struct CustomGeometrySourceType;

/// A custom geometry source's options.
#[derive(Clone)]
pub struct CustomGeometrySourceOptions {
    pub tile_options: Arc<TileOptions>,
    pub zoom_range: Range<u8>,
    /// The host's functions that give the tiles' GeoJSON.
    pub tile_functions: Arc<TileFunctions<GeoJson>>,
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
    const INFO: SourceTypeInfo = SourceTypeInfo {
        name: "custom-geometry",
        layer_kind: SourceKind::Geometry,
        from_json: false,
        round_zoom: false,
        overscale: true,
        prefetch: true,
    };

    type Options = CustomGeometrySourceOptions;
    /// None: the host's tile functions give the source's data.
    type Description = ();
}

impl SourceData<CustomGeometrySourceType> {
    /// Whether tiles made for one source don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Self) -> bool {
        !Arc::ptr_eq(&self.options.tile_options, &other.options.tile_options)
            || self.options.zoom_range != other.options.zoom_range
            || !Arc::ptr_eq(&self.options.tile_functions, &other.options.tile_functions)
    }
}

/// The custom vector source type, whose tiles' encoded bytes the host gives.
pub struct CustomVectorSourceType;

/// A custom vector source's options.
#[derive(Clone)]
pub struct CustomVectorSourceOptions {
    pub zoom_range: Range<u8>,
    /// The host's functions that give the tiles' bytes.
    pub tile_functions: Arc<TileFunctions<CustomVectorTileData>>,
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

impl SourceData<CustomVectorSourceType> {
    /// Whether tiles made for one source don't suit the other.
    #[must_use]
    pub fn differs(&self, other: &Self) -> bool {
        self.options.zoom_range != other.options.zoom_range
            || !Arc::ptr_eq(&self.options.tile_functions, &other.options.tile_functions)
    }
}
