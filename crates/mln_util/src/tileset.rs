// Ports include/mln/util/tileset.hpp (the `Scheme` that Resource::tile
// takes).
//
// TODO(port): `Tileset` itself (tiles, zoom range, attribution, scheme,
// encodings, bounds) with the tile sources (M4).

use serde::{Deserialize, Serialize};

/// `Tileset::Scheme`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scheme {
    #[default]
    XYZ,
    TMS,
}

/// `Tileset::RasterEncoding`. (Added by `sources` with the tile sources.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RasterEncoding {
    /// Mapbox DEM
    Mapbox,
    /// Terrarium DEM
    Terrarium,
}

/// `Tileset::VectorEncoding`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorEncoding {
    /// Mapbox Vector Tiles (MVT)
    Mapbox,
    /// MapLibre Tiles
    MLT,
}

/// `Tileset`. TileJSON also includes center and zoom but they are not used
/// by mbgl.
#[derive(Clone, Debug, PartialEq)]
pub struct Tileset {
    pub tiles: Vec<String>,
    pub zoom_range: crate::range::Range<u8>,
    pub attribution: String,
    pub scheme: Scheme,
    pub raster_encoding: Option<RasterEncoding>,
    pub vector_encoding: Option<VectorEncoding>,
    pub bounds: Option<crate::geo::LatLngBounds>,
}

impl Default for Tileset {
    /// `Tileset(tiles = {}, zoomRange = {0, DEFAULT_MAX_ZOOM}, attribution =
    /// {}, scheme = XYZ, rasterEncoding = nullopt, encoding = nullopt)`.
    fn default() -> Self {
        Tileset {
            tiles: Vec::new(),
            zoom_range: crate::range::Range::new(0, crate::constants::DEFAULT_MAX_ZOOM),
            attribution: String::new(),
            scheme: Scheme::XYZ,
            raster_encoding: None,
            vector_encoding: None,
            bounds: None,
        }
    }
}
