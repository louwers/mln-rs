//! The snapshot's tile sets: which tiles each holds, and their archives.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pmtiles::{Compression, PmTilesReader, PmTilesWriter, TileCoord, TileId, TileType};
use serde_json::{Value, json};

/// A tile set: a pyramid over an area, or the whole world.
pub(crate) struct TileSet {
    /// The archive's name, `tiles/<name>.pmtiles`.
    pub(crate) name: &'static str,
    /// West, south, east, north, in degrees. A camera centered in it sees
    /// only tiles of the set.
    pub(crate) area: [f64; 4],
    pub(crate) max_zoom: u8,
    /// What the tiles are for, for the manifest.
    pub(crate) description: &'static str,
}

/// Central San Francisco: the city, wide enough for a camera to fly over.
pub(crate) const STREET: TileSet = TileSet {
    name: "street",
    area: [-122.515, 37.705, -122.355, 37.815],
    max_zoom: 14,
    description: "OpenMapTiles vector tiles of central San Francisco, zooms 0 to 14",
};

/// The whole world, for the globe.
pub(crate) const WORLD: TileSet = TileSet {
    name: "world",
    area: [-180.0, -85.051_128_779_806_59, 180.0, 85.051_128_779_806_59],
    max_zoom: 4,
    description: "OpenMapTiles vector tiles of the whole world, zooms 0 to 4",
};

/// Elevation over the street set's area, for hillshading and color relief.
pub(crate) const TERRAIN: TileSet = TileSet {
    name: "terrain",
    area: STREET.area,
    max_zoom: 12,
    description: "Mapterhorn's terrain tiles of central San Francisco, Terrarium-encoded WebP of 512 pixels, zooms 0 to 12",
};

/// Central Tokyo, whose names are in Japanese script.
pub(crate) const CJK: TileSet = TileSet {
    name: "cjk",
    area: [139.74, 35.665, 139.79, 35.70],
    max_zoom: 14,
    description: "OpenMapTiles vector tiles of central Tokyo, zooms 0 to 14, with names in Japanese script",
};

impl TileSet {
    /// Every tile of the set, by zoom: at each zoom, the tiles over the area
    /// and one more on each side, so that a camera centered in the area sees
    /// no tile the set lacks when its view is at most two tiles across.
    pub(crate) fn tiles(&self) -> Vec<TileCoord> {
        let mut tiles = Vec::new();
        for z in 0..=self.max_zoom {
            let ([x0, y0], [x1, y1]) = self.range(z);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    tiles.push(TileCoord::new(z, x, y).expect("a tile in the world"));
                }
            }
        }
        tiles
    }

    /// The first and last column and row at zoom `z`.
    fn range(&self, z: u8) -> ([u32; 2], [u32; 2]) {
        let [west, south, east, north] = self.area;
        let last = (1u32 << z) - 1;
        let (x0, y0) = tile(west, north, z);
        let (x1, y1) = tile(east, south, z);
        ([x0.saturating_sub(1), y0.saturating_sub(1)], [(x1 + 1).min(last), (y1 + 1).min(last)])
    }

    /// West, south, east and north of the set's tiles at its last zoom: the
    /// archive's bounds, outside which a map requests no tile.
    pub(crate) fn bounds(&self) -> [f64; 4] {
        let ([x0, y0], [x1, y1]) = self.range(self.max_zoom);
        let (west, north) = corner(x0, y0, self.max_zoom);
        let (east, south) = corner(x1 + 1, y1 + 1, self.max_zoom);
        [west, south, east, north]
    }

    pub(crate) fn center(&self) -> [f64; 2] {
        let [west, south, east, north] = self.area;
        [f64::midpoint(west, east), f64::midpoint(south, north)]
    }
}

/// The tile at zoom `z` that holds a point.
fn tile(lon: f64, lat: f64, z: u8) -> (u32, u32) {
    let n = f64::from(1u32 << z);
    let lat = lat.to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n;
    let last = n - 1.0;
    (x.floor().clamp(0.0, last) as u32, y.floor().clamp(0.0, last) as u32)
}

/// The north-west corner of tile `x`, `y` at zoom `z`, as longitude and latitude.
fn corner(x: u32, y: u32, z: u8) -> (f64, f64) {
    let n = f64::from(1u32 << z);
    let lon = f64::from(x) / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * f64::from(y) / n)).sinh().atan().to_degrees();
    (lon, lat)
}

/// How an archive's tiles are encoded.
#[derive(Clone, Copy)]
pub(crate) enum Encoding {
    /// Mapbox Vector Tiles, gzipped as tile servers serve them.
    Mvt,
    /// MapLibre Tiles, stored as they are.
    Mlt,
    /// WebP images, stored as they are.
    Webp,
}

/// Writes an archive of `tiles` (uncompressed), with `metadata`, for `set`.
pub(crate) fn write_archive(
    path: &Path,
    set: &TileSet,
    encoding: Encoding,
    tiles: &BTreeMap<TileId, Vec<u8>>,
    metadata: &Value,
) -> Result<(), String> {
    let error = |error: pmtiles::PmtError| format!("{}: {error}", path.display());
    let file = std::fs::File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (tile_type, compression) = match encoding {
        Encoding::Mvt => (TileType::Mvt, Compression::Gzip),
        Encoding::Mlt => (TileType::Mlt, Compression::None),
        Encoding::Webp => (TileType::Webp, Compression::None),
    };
    let [west, south, east, north] = set.bounds();
    let [lon, lat] = set.center();
    let mut writer = PmTilesWriter::new(tile_type)
        .tile_compression(compression)
        .min_zoom(0)
        .max_zoom(set.max_zoom)
        .bounds(west, south, east, north)
        .center(lon, lat)
        .center_zoom(set.max_zoom.min(12))
        .metadata(metadata.to_string())
        .create(std::io::BufWriter::new(file))
        .map_err(error)?;
    for (&id, data) in tiles {
        writer.add_tile(TileCoord::from(id), data).map_err(error)?;
    }
    writer.finalize().map_err(error)
}

/// Reads an archive back, and checks that it holds each of `expected`'s
/// tiles with the same bytes.
pub(crate) fn check_archive(path: &Path, expected: &BTreeMap<TileId, Vec<u8>>) -> Result<(), String> {
    let error = |error: pmtiles::PmtError| format!("{}: {error}", path.display());
    let reader = PmTilesReader::new_with_path(path).map_err(error)?;
    for (&id, data) in expected {
        let tile = reader.get_tile_decompressed(id).map_err(error)?;
        if tile.as_deref() != Some(data.as_slice()) {
            return Err(format!("{}: tile {:?} doesn't read back", path.display(), TileCoord::from(id)));
        }
    }
    Ok(())
}

/// The names of an MVT tile's layers.
pub(crate) fn mvt_layers(data: &[u8]) -> Result<BTreeSet<String>, String> {
    let layers = mlt_core::mvt::mvt_to_tile_layers(data).map_err(|error| error.to_string())?;
    Ok(layers.iter().map(|layer| layer.name().to_owned()).collect())
}

/// The names of an MLT tile's layers.
pub(crate) fn mlt_layers(data: &[u8]) -> Result<BTreeSet<String>, String> {
    let mut parser = mlt_core::Parser::default();
    let layers = parser.parse_layers(data).map_err(|error| error.to_string())?;
    Ok(layers.iter().filter_map(mlt_core::Layer::name).map(str::to_owned).collect())
}

/// `tile` as MLT, every polygon with its triangles, as MapLibre Native
/// draws them, and no FastPFOR, which mln-rs reads only when a map enables it.
pub(crate) fn encode_mlt(mvt: &[u8]) -> Result<Vec<u8>, String> {
    use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};

    let config = EncoderConfig::from_profile(Profile::Native(Network::Offline))
        .with_tessellation(Tessellation::Always)
        .with_fastpfor(false);
    let mut mlt = Vec::new();
    for layer in mlt_core::mvt::mvt_to_tile_layers(mvt).map_err(|error| error.to_string())? {
        mlt.extend(layer.encode(config).map_err(|error| error.to_string())?);
    }
    Ok(mlt)
}

/// The metadata of an archive: the planet's TileJSON's description of its
/// layers and attribution.
pub(crate) fn metadata(set: &TileSet, encoding: Encoding, tilejson: &Value) -> Value {
    let name = match encoding {
        Encoding::Mvt | Encoding::Webp => set.name.to_owned(),
        Encoding::Mlt => format!("{}-mlt", set.name),
    };
    json!({
        "name": name,
        "description": set.description,
        "attribution": tilejson["attribution"],
        "version": tilejson["version"],
        "vector_layers": tilejson["vector_layers"],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_set_covers_its_area_and_one_tile_more() {
        assert_eq!(tile(-122.42, 37.77, 14), (2620, 6333));
        let tiles = STREET.tiles();
        assert!(tiles.contains(&TileCoord::new(0, 0, 0).unwrap()));
        assert!(tiles.contains(&TileCoord::new(14, 2620, 6333).unwrap()));
        // Zoom 1 has 4 tiles; a set never leaves the world.
        assert_eq!(WORLD.tiles().len(), 1 + 4 + 16 + 64 + 256);
        let [west, south, east, north] = STREET.bounds();
        assert!(west < STREET.area[0] && east > STREET.area[2] && south < STREET.area[1] && north > STREET.area[3]);
    }
}
