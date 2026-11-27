//! Cuts GeoJSON features into vector tiles, as geojson-vt does.
//!
//! [`TileIndex`] projects the features onto Web Mercator's unit square once,
//! simplifies their lines for its maximum zoom and ranks each point by how
//! much it matters, so that a tile at any zoom keeps the points that matter
//! at that zoom. It cuts tiles down to a shallow zoom up front, and deeper
//! ones on request, from the nearest tile above that it kept the features
//! of. [`cut_tile`] cuts one tile without an index.

mod clip;
mod convert;
mod geometry;
mod output;
mod simplify;

use std::sync::Arc;

use clip::clip as clip_axis;
use geometry::{Bbox, Feature, X, Y};
use output::Tile;
use parking_lot::{Mutex, RwLock};

use crate::TileFeatures;

/// foldhash's hasher, with a fixed seed in the benchmarks' builds, as
/// `mln_style::hash`'s.
#[cfg(not(mln_bench_fixed_hash))]
type Hasher = foldhash::fast::RandomState;
#[cfg(mln_bench_fixed_hash)]
type Hasher = foldhash::fast::FixedState;

#[expect(clippy::disallowed_types, reason = "mln_style::hash's map, which this crate doesn't depend on")]
type HashMap<K, V> = std::collections::HashMap<K, V, Hasher>;

/// How tiles are cut and written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileOptions {
    /// How far simplifying may move a line, in tile units.
    pub tolerance: f64,
    /// The size of a tile, in tile units.
    pub extent: u16,
    /// How far past its edges a tile has its features, in tile units.
    pub buffer: u16,
    /// Whether a line's pieces say where along the line they start and end,
    /// for line gradients.
    pub line_metrics: bool,
}

impl Default for TileOptions {
    fn default() -> Self {
        TileOptions { tolerance: 3.0, extent: 4096, buffer: 64, line_metrics: false }
    }
}

/// How a [`TileIndex`] cuts tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub tile: TileOptions,
    /// The deepest zoom with tiles, at most [`MAX_ZOOM`]. Its tiles keep
    /// every point.
    pub max_zoom: u8,
    /// The deepest zoom that the index cuts up front.
    pub index_max_zoom: u8,
    /// The most points that a tile cut up front keeps without being cut
    /// further.
    pub index_max_points: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options { tile: TileOptions::default(), max_zoom: 18, index_max_zoom: 5, index_max_points: 100_000 }
    }
}

/// The deepest [`Options::max_zoom`]. A deeper one counts as this.
pub const MAX_ZOOM: u8 = 31;

/// Why a tile can't be cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("tile zoom {0} is past the maximum zoom")]
    ZoomPastMax(u8),
    #[error("tile row {y} is past the last row of zoom {z}")]
    RowPastEnd { z: u8, y: u32 },
}

/// A tile's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileId {
    z: u8,
    x: u32,
    y: u32,
}

impl TileId {
    const ROOT: TileId = TileId { z: 0, x: 0, y: 0 };

    fn parent(self) -> Option<TileId> {
        (self.z > 0).then(|| TileId { z: self.z - 1, x: self.x / 2, y: self.y / 2 })
    }

    fn child(self, dx: u32, dy: u32) -> TileId {
        TileId { z: self.z + 1, x: self.x * 2 + dx, y: self.y * 2 + dy }
    }

    /// The tile below `self` on the way to `other`, which is below it.
    fn child_toward(self, other: TileId) -> TileId {
        let shift = other.z - self.z - 1;
        TileId { z: self.z + 1, x: other.x >> shift, y: other.y >> shift }
    }

    /// Whether `self` is `other` or above it.
    fn contains(self, other: TileId) -> bool {
        self.z <= other.z && other.x >> (other.z - self.z) == self.x && other.y >> (other.z - self.z) == self.y
    }
}

/// A tile that the index cut.
struct Node {
    features: Arc<TileFeatures>,
    bbox: Bbox,
    /// The features it was cut from, while the tiles below it aren't cut.
    /// Cutting them holds the lock, so that they're cut once.
    source: Mutex<Vec<Feature>>,
}

/// GeoJSON features, cut into tiles up to a maximum zoom. Tiles can be cut
/// from several threads at once.
pub struct TileIndex {
    options: Options,
    tiles: RwLock<HashMap<TileId, Arc<Node>>>,
}

impl std::fmt::Debug for TileIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TileIndex").field("options", &self.options).finish_non_exhaustive()
    }
}

impl TileIndex {
    /// Indexes `features` (in longitude and latitude) for tiles.
    #[must_use]
    pub fn new(features: &[mln_style::feature::Feature<f64>], options: Options) -> Self {
        let options = Options { max_zoom: options.max_zoom.min(MAX_ZOOM), ..options };
        let tile = &options.tile;
        let tolerance = tile.tolerance / f64::from(tile.extent) / 2f64.powi(i32::from(options.max_zoom));
        let features = convert::convert(features, tolerance, tile.line_metrics);
        let features = clip::wrap(features, f64::from(tile.buffer) / f64::from(tile.extent), tile.line_metrics);
        let index = TileIndex { options, tiles: RwLock::new(HashMap::default()) };
        index.add(&features, TileId::ROOT, None);
        index
    }

    /// The features of tile `x`, `y` at zoom `z`, cut now if they aren't
    /// yet. Columns wrap around the world.
    pub fn tile(&self, z: u8, x: u32, y: u32) -> Result<Arc<TileFeatures>, Error> {
        if z > self.options.max_zoom {
            return Err(Error::ZoomPastMax(z));
        }
        let z2 = 1u64 << z;
        if u64::from(y) >= z2 {
            return Err(Error::RowPastEnd { z, y });
        }
        // `x % z2` is less than `z2`, at most 2^31.
        let id = TileId { z, x: (u64::from(x) % z2) as u32, y };

        loop {
            let (ancestor, node) = {
                let tiles = self.tiles.read();
                if let Some(node) = tiles.get(&id) {
                    return Ok(Arc::clone(&node.features));
                }
                // The root tile is always there.
                let Some(nearest) = std::iter::successors(id.parent(), |t| t.parent())
                    .find_map(|t| tiles.get(&t).map(|node| (t, Arc::clone(node))))
                else {
                    return Ok(Arc::default());
                };
                nearest
            };
            let mut source = node.source.lock();
            // Another request may have cut the tiles below while this one
            // waited for the lock: then it starts over from them.
            if self.tiles.read().contains_key(&ancestor.child_toward(id)) {
                continue;
            }
            let features = std::mem::take(&mut *source);
            if !features.is_empty() {
                self.cut_below(&features, ancestor, node.bbox, Some(id));
            }
            drop(source);
            return Ok(self.tiles.read().get(&id).map_or_else(Arc::default, |node| Arc::clone(&node.features)));
        }
    }

    /// The tiles cut so far.
    #[cfg(test)]
    fn tile_ids(&self) -> Vec<TileId> {
        self.tiles.read().keys().copied().collect()
    }

    /// Adds tile `id`, cut from `features`, and cuts the tiles below it: down
    /// to `target` with one, or down to the index's limits without.
    fn add(&self, features: &[Feature], id: TileId, target: Option<TileId>) {
        let options = &self.options;
        let tolerance = if id.z == options.max_zoom {
            0.0
        } else {
            options.tile.tolerance / (2f64.powi(i32::from(id.z)) * f64::from(options.tile.extent))
        };
        let tile = Tile::new(features, id, options.tile.extent, tolerance, options.tile.line_metrics);
        let mut bbox = Bbox::EMPTY;
        for feature in features {
            bbox.extend(feature.bbox);
        }

        let deeper = !features.is_empty() && id.z < options.max_zoom;
        let cut_below = deeper
            && match target {
                None => id.z < options.index_max_zoom && tile.num_points > options.index_max_points,
                Some(target) => id.z < target.z && id.contains(target),
            };
        let source = if deeper && !cut_below { features.to_vec() } else { Vec::new() };
        let node = Arc::new(Node { features: Arc::new(tile.features), bbox, source: Mutex::new(source) });
        // A request that finds the tile before the tiles below it are cut
        // waits for them.
        let cutting = cut_below.then(|| node.source.lock());
        self.tiles.write().insert(id, Arc::clone(&node));
        if cut_below {
            self.cut_below(features, id, bbox, target);
        }
        drop(cutting);
    }

    /// Cuts the four tiles below tile `id` from `features`, its features,
    /// which `bbox` bounds.
    fn cut_below(&self, features: &[Feature], id: TileId, bbox: Bbox, target: Option<TileId>) {
        let line_metrics = self.options.tile.line_metrics;
        let z2 = 2f64.powi(i32::from(id.z));
        let p = 0.5 * f64::from(self.options.tile.buffer) / f64::from(self.options.tile.extent);
        for dx in 0..2 {
            let x = f64::from(id.x) + 0.5 * f64::from(dx);
            let column =
                clip_axis::<X>(features, (x - p) / z2, (x + 0.5 + p) / z2, bbox.min.x, bbox.max.x, line_metrics);
            for dy in 0..2 {
                let y = f64::from(id.y) + 0.5 * f64::from(dy);
                let quadrant =
                    clip_axis::<Y>(&column, (y - p) / z2, (y + 0.5 + p) / z2, bbox.min.y, bbox.max.y, line_metrics);
                self.add(&quadrant, id.child(dx, dy), target);
            }
        }
    }
}

/// Cuts tile `x`, `y` at zoom `z` from `features` (in longitude and
/// latitude), without an index. `wrap` also gives it what lies past the
/// antimeridian, and `clip` cuts away what lies past its buffer.
#[must_use]
pub fn cut_tile(
    features: &[mln_style::feature::Feature<f64>],
    z: u8,
    x: u32,
    y: u32,
    options: &TileOptions,
    wrap: bool,
    clip: bool,
) -> TileFeatures {
    let z2 = 2f64.powi(i32::from(z));
    let tolerance = options.tolerance / f64::from(options.extent) / z2;
    let buffer = f64::from(options.buffer) / f64::from(options.extent);
    let mut features = convert::convert(features, tolerance, options.line_metrics);
    if wrap {
        features = clip::wrap(features, buffer, options.line_metrics);
    }
    if clip || options.line_metrics {
        let (x, y) = (f64::from(x), f64::from(y));
        let column =
            clip_axis::<X>(&features, (x - buffer) / z2, (x + 1.0 + buffer) / z2, -1.0, 2.0, options.line_metrics);
        features = clip_axis::<Y>(&column, (y - buffer) / z2, (y + 1.0 + buffer) / z2, -1.0, 2.0, options.line_metrics)
            .into_owned();
    }
    Tile::new(&features, TileId { z, x, y }, options.extent, tolerance, options.line_metrics).features
}

#[cfg(test)]
mod tests;
