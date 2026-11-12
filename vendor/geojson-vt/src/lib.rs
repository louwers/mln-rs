// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt.hpp
// (mapbox::geojsonvt: GeoJSONVT, Options, TileOptions, geoJSONToTile) and
// its detail headers (types, convert, simplify, clip, wrap, tile).
//
// `geojsonvt::geojson` is the same variant as `mapbox::geojson::geojson`
// (`mln::GeoJSON`): //vendor/geojson's `GeoJSON`.

pub mod clip;
pub mod convert;
pub mod simplify;
pub mod tile;
pub mod types;
pub mod wrap;

/// `mapbox::geojsonvt::detail`
pub mod detail {
    pub use crate::clip::{Clipper, clip};
    pub use crate::convert::{Project, convert};
    pub use crate::simplify::{get_sq_seg_dist, simplify, simplify_range};
    pub use crate::tile::InternalTile;
    pub use crate::types::*;
    pub use crate::wrap::{shift_coords, wrap};
}

use std::collections::{BTreeMap, HashMap};

use detail::{InternalTile, VtFeatures};
use feature::Feature;
use geojson::{FeatureCollection, GeoJSON};
pub use tile::Tile;

/// `ToFeatureCollection`
#[must_use]
pub fn to_feature_collection(geojson: &GeoJSON) -> FeatureCollection<f64> {
    match geojson {
        GeoJSON::FeatureCollection(value) => value.clone(),
        GeoJSON::Feature(value) => vec![value.clone()],
        GeoJSON::Geometry(value) => vec![Feature::new(value.clone())],
    }
}

/// `TileOptions`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileOptions {
    /// simplification tolerance (higher means simpler)
    pub tolerance: f64,
    /// tile extent
    pub extent: u16,
    /// tile buffer on each side
    pub buffer: u16,
    /// enable line metrics tracking for LineString/MultiLineString features
    pub line_metrics: bool,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tolerance: 3.0, extent: 4096, buffer: 64, line_metrics: false }
    }
}

/// `Options` (derives from `TileOptions` in C++).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub tile: TileOptions,
    /// max zoom to preserve detail on
    pub max_zoom: u8,
    /// max zoom in the tile index
    pub index_max_zoom: u8,
    /// max number of points per tile in the tile index
    pub index_max_points: u32,
    /// whether to generate feature ids, overriding existing ids
    pub generate_id: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tile: TileOptions::default(),
            max_zoom: 18,
            index_max_zoom: 5,
            index_max_points: 100_000,
            generate_id: false,
        }
    }
}

/// `empty_tile`
pub static EMPTY_TILE: Tile = Tile { features: Vec::new(), num_points: 0, num_simplified: 0 };

/// `toID(z, x, y)`
#[must_use]
pub fn to_id(z: u8, x: u32, y: u32) -> u64 {
    1u64.wrapping_shl(u32::from(z))
        .wrapping_mul(u64::from(y))
        .wrapping_add(u64::from(x))
        .wrapping_mul(32)
        .wrapping_add(u64::from(z))
}

/// `geoJSONToTile(geojson, z, x, y, options, wrap, clip)`
#[must_use]
pub fn geojson_to_tile(
    geojson: &GeoJSON,
    z: u8,
    x: u32,
    y: u32,
    options: &TileOptions,
    wrap: bool,
    clip: bool,
) -> Tile {
    let features_ = to_feature_collection(geojson);
    let z2 = 1u32.wrapping_shl(u32::from(z));
    let tolerance = (options.tolerance / f64::from(options.extent)) / f64::from(z2);
    let mut features = detail::convert(&features_, tolerance, false);
    if wrap {
        features = detail::wrap(&features, f64::from(options.buffer) / f64::from(options.extent), options.line_metrics);
    }
    if clip || options.line_metrics {
        let p = f64::from(options.buffer) / f64::from(options.extent);
        let z2 = f64::from(z2);

        let left = detail::clip::<0>(
            &features,
            (f64::from(x) - p) / z2,
            (f64::from(x.wrapping_add(1)) + p) / z2,
            -1.0,
            2.0,
            options.line_metrics,
        );
        features = detail::clip::<1>(
            &left,
            (f64::from(y) - p) / z2,
            (f64::from(y.wrapping_add(1)) + p) / z2,
            -1.0,
            2.0,
            options.line_metrics,
        );
    }
    InternalTile::new(&features, z, x, y, options.extent, tolerance, options.line_metrics).tile
}

/// `GeoJSONVT`
pub struct GeoJSONVT {
    pub options: Options,
    pub stats: BTreeMap<u8, u32>,
    pub total: u32,
    // C++: `std::unordered_map`; only looked up, and iterated by
    // `getInternalTiles()` users that do not depend on the order.
    tiles: HashMap<u64, InternalTile>,
}

impl GeoJSONVT {
    /// `GeoJSONVT(const feature_collection&, const Options&)`
    #[must_use]
    pub fn new(features: &FeatureCollection<f64>, options: Options) -> Self {
        let mut vt = GeoJSONVT { options, stats: BTreeMap::new(), total: 0, tiles: HashMap::new() };

        let z2 = 1u32.wrapping_shl(u32::from(options.max_zoom));

        let converted = detail::convert(
            features,
            (options.tile.tolerance / f64::from(options.tile.extent)) / f64::from(z2),
            options.generate_id,
        );
        let features = detail::wrap(
            &converted,
            f64::from(options.tile.buffer) / f64::from(options.tile.extent),
            options.tile.line_metrics,
        );

        vt.split_tile(&features, 0, 0, 0, 0, 0, 0);
        vt
    }

    /// `GeoJSONVT(const geojson&, const Options&)`
    #[must_use]
    pub fn from_geojson(geojson: &GeoJSON, options: Options) -> Self {
        Self::new(&to_feature_collection(geojson), options)
    }

    /// `getTile(z, x, y)`; `Err` is the C++ `std::runtime_error` message.
    pub fn get_tile(&mut self, z: u8, x_: u32, y: u32) -> Result<&Tile, String> {
        if z > self.options.max_zoom {
            return Err(format!("Requested zoom higher than maxZoom: {z}"));
        }

        let z2 = 1u32.wrapping_shl(u32::from(z));
        let x = ((x_ % z2).wrapping_add(z2)) % z2; // wrap tile x coordinate
        let id = to_id(z, x, y);

        if self.tiles.contains_key(&id) {
            return Ok(&self.tiles[&id].tile);
        }

        let Some(parent_id) = self.find_parent(z, x, y) else {
            return Err("Parent tile not found".to_owned());
        };

        // if we found a parent tile containing the original geometry, we can
        // drill down from it
        let parent = &self.tiles[&parent_id];
        let (source_features, pz, px, py) = (parent.source_features.clone(), parent.z, parent.x, parent.y);

        // drill down parent tile up to the requested one
        self.split_tile(&source_features, pz, px, py, z, x, y);

        if self.tiles.contains_key(&id) {
            return Ok(&self.tiles[&id].tile);
        }

        if self.find_parent(z, x, y).is_none() {
            return Err("Parent tile not found".to_owned());
        }

        Ok(&EMPTY_TILE)
    }

    /// `getInternalTiles()`
    #[must_use]
    pub fn get_internal_tiles(&self) -> &HashMap<u64, InternalTile> {
        &self.tiles
    }

    /// `findParent(z, x, y)`: the id of the nearest ancestor tile.
    fn find_parent(&self, z: u8, x: u32, y: u32) -> Option<u64> {
        let mut z0 = z;
        let mut x0 = x;
        let mut y0 = y;

        while z0 != 0 {
            z0 -= 1;
            x0 /= 2;
            y0 /= 2;
            let id = to_id(z0, x0, y0);
            if self.tiles.contains_key(&id) {
                return Some(id);
            }
        }

        None
    }

    #[allow(clippy::too_many_arguments)]
    fn split_tile(&mut self, features: &VtFeatures, z: u8, x: u32, y: u32, cz: u8, cx: u32, cy: u32) {
        let options = self.options;
        let z2 = f64::from(1u32.wrapping_shl(u32::from(z)));
        let id = to_id(z, x, y);

        if let std::collections::hash_map::Entry::Vacant(e) = self.tiles.entry(id) {
            let tolerance = if z == options.max_zoom {
                0.0
            } else {
                options.tile.tolerance / (z2 * f64::from(options.tile.extent))
            };

            e.insert(InternalTile::new(features, z, x, y, options.tile.extent, tolerance, options.tile.line_metrics));
            let count = self.stats.get(&z).map_or(1, |c| c.wrapping_add(1));
            self.stats.insert(z, count);
            self.total = self.total.wrapping_add(1);
        }

        let tile = self.tiles.get_mut(&id).unwrap();

        if features.is_empty() {
            return;
        }

        // if it's the first-pass tiling
        if cz == 0 {
            // stop tiling if we reached max zoom, or if the tile is too simple
            if z == options.index_max_zoom || tile.tile.num_points <= options.index_max_points {
                tile.source_features = features.clone();
                return;
            }
        } else {
            // drilldown to a specific tile;
            // stop tiling if we reached base zoom
            if z == options.max_zoom {
                return;
            }

            // stop tiling if it's our target tile zoom
            if z == cz {
                tile.source_features = features.clone();
                return;
            }

            // stop tiling if it's not an ancestor of the target tile
            let m = f64::from(1u32.wrapping_shl(u32::from(cz - z)));
            if x != (f64::from(cx) / m).floor() as u32 || y != (f64::from(cy) / m).floor() as u32 {
                tile.source_features = features.clone();
                return;
            }
        }

        let p = 0.5 * f64::from(options.tile.buffer) / f64::from(options.tile.extent);
        let min = tile.bbox.min;
        let max = tile.bbox.max;
        let lm = options.tile.line_metrics;
        let (xf, yf) = (f64::from(x), f64::from(y));

        let left = detail::clip::<0>(features, (xf - p) / z2, (xf + 0.5 + p) / z2, min.x, max.x, lm);

        self.split_tile(
            &detail::clip::<1>(&left, (yf - p) / z2, (yf + 0.5 + p) / z2, min.y, max.y, lm),
            z.wrapping_add(1),
            x.wrapping_mul(2),
            y.wrapping_mul(2),
            cz,
            cx,
            cy,
        );
        self.split_tile(
            &detail::clip::<1>(&left, (yf + 0.5 - p) / z2, (yf + 1.0 + p) / z2, min.y, max.y, lm),
            z.wrapping_add(1),
            x.wrapping_mul(2),
            y.wrapping_mul(2).wrapping_add(1),
            cz,
            cx,
            cy,
        );

        let right = detail::clip::<0>(features, (xf + 0.5 - p) / z2, (xf + 1.0 + p) / z2, min.x, max.x, lm);

        self.split_tile(
            &detail::clip::<1>(&right, (yf - p) / z2, (yf + 0.5 + p) / z2, min.y, max.y, lm),
            z.wrapping_add(1),
            x.wrapping_mul(2).wrapping_add(1),
            y.wrapping_mul(2),
            cz,
            cx,
            cy,
        );
        self.split_tile(
            &detail::clip::<1>(&right, (yf + 0.5 - p) / z2, (yf + 1.0 + p) / z2, min.y, max.y, lm),
            z.wrapping_add(1),
            x.wrapping_mul(2).wrapping_add(1),
            y.wrapping_mul(2).wrapping_add(1),
            cz,
            cx,
            cy,
        );

        // if we sliced further down, no need to keep source geometry
        self.tiles.get_mut(&id).unwrap().source_features = VtFeatures::new();
    }
}
