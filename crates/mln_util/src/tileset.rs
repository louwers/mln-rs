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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RasterEncoding {
    /// Mapbox DEM
    Mapbox,
    /// Terrarium DEM
    Terrarium,
    /// The style spec's `custom` DEM: a pixel's elevation is
    /// `r * red_factor + g * green_factor + b * blue_factor - base_shift`,
    /// with each channel from 0 to 255.
    Custom { red_factor: f32, green_factor: f32, blue_factor: f32, base_shift: f32 },
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
    /// The style vector source's `sparse`; a TileJSON doesn't set it. When it's set, an empty tile
    /// response (HTTP 204, or no tile data from an archive such as PMTiles) means the tileset has
    /// no tile there at that zoom level, and a tile from a lower zoom level may be drawn in its
    /// place. Otherwise it's a tile with nothing at that place.
    pub sparse: bool,
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
            sparse: false,
        }
    }
}
