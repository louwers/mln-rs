use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Scheme {
    #[default]
    XYZ,
    TMS,
}

/// How a raster DEM tileset encodes elevation in its pixels.
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
    /// The source's `sparse`. When it's set, an empty tile
    /// response (HTTP 204, or no tile data from an archive such as PMTiles) means the tileset has
    /// no tile there at that zoom level, and a tile from a lower zoom level may be drawn in its
    /// place. Otherwise it's a tile with nothing at that place.
    pub sparse: bool,
}

impl Default for Tileset {
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

/// A tileset's members as a TileJSON document or a style source lists them, each optional, so
/// that a style source's members can win over those of the TileJSON at its URL.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartialTileset {
    pub tiles: Option<Vec<String>>,
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    pub attribution: Option<String>,
    pub scheme: Option<Scheme>,
    pub raster_encoding: Option<RasterEncoding>,
    pub vector_encoding: Option<VectorEncoding>,
    pub bounds: Option<crate::geo::LatLngBounds>,
    pub sparse: Option<bool>,
}

impl PartialTileset {
    /// `self`'s members, with `fallback`'s in place of those `self` doesn't have.
    #[must_use]
    pub fn or(self, fallback: PartialTileset) -> PartialTileset {
        PartialTileset {
            tiles: self.tiles.or(fallback.tiles),
            min_zoom: self.min_zoom.or(fallback.min_zoom),
            max_zoom: self.max_zoom.or(fallback.max_zoom),
            attribution: self.attribution.or(fallback.attribution),
            scheme: self.scheme.or(fallback.scheme),
            raster_encoding: self.raster_encoding.or(fallback.raster_encoding),
            vector_encoding: self.vector_encoding.or(fallback.vector_encoding),
            bounds: self.bounds.or(fallback.bounds),
            sparse: self.sparse.or(fallback.sparse),
        }
    }
}

/// The tileset, with [`Tileset::default`]'s members in place of those it doesn't have.
impl From<PartialTileset> for Tileset {
    fn from(partial: PartialTileset) -> Self {
        let default = Tileset::default();
        Tileset {
            tiles: partial.tiles.unwrap_or_default(),
            zoom_range: crate::range::Range::new(
                partial.min_zoom.unwrap_or(default.zoom_range.min),
                partial.max_zoom.unwrap_or(default.zoom_range.max),
            ),
            attribution: partial.attribution.unwrap_or_default(),
            scheme: partial.scheme.unwrap_or_default(),
            raster_encoding: partial.raster_encoding,
            vector_encoding: partial.vector_encoding,
            bounds: partial.bounds,
            sparse: partial.sparse.unwrap_or_default(),
        }
    }
}
