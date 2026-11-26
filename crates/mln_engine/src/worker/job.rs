//! The worker contract: a tile's job ([`Run`]) and its one reply
//! ([`Reply`]), and [`run_job`], which every transport runs.
//!
//! A job carries every input it reads, so a worker keeps nothing between
//! jobs but a cache of tile data (web only), whose miss the job answers
//! with [`JobResult::NeedsData`]. A layout whose dependencies its job's
//! [`Resolved`] doesn't cover answers [`JobResult::NeedsDeps`] after
//! discovery; the tile resolves them and runs the job again, which
//! discovers again and builds. A raster or raster-dem tile's job decodes
//! its image.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mln_image::decoder;
use mln_style::bitmap::PremultipliedImage;
use mln_style::error::Error;
use mln_style::expression::GlobalStateMap;
use mln_style::hash::HashMap;
use mln_style::tile_id::CanonicalTileID;
use mln_style::tileset::RasterEncoding;
use mln_style::time::Instant;
use mln_text::glyph::FontFaces;
use mln_vec_map::VecSet;
use mln_wire::payload::shared_bytes;
use mln_wire::shared;
use parking_lot::Mutex;

use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::layer::tile_layout::{Dependencies, Discovered, LayoutInput, LayoutParams, Resolved, build, discover};
use crate::tile::dem_data::DEMData;
use crate::tile::{OverscaledTileID, TileData};
use crate::tile_store::contents::LayoutResult;
use crate::worker::worker_codec::fields::{SharedAvailableImages, SharedSourceStyle};

/// A source, as the renderer numbers it. Never reused within a renderer, so
/// a late reply can't match a source that was removed and added again.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, mln_wire::Wire)]
pub(crate) struct SourceKey(pub(crate) u64);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct TileKey {
    pub(crate) source: SourceKey,
    pub(crate) id: OverscaledTileID,
}

/// One generation of a tile's inputs. Generations come from one counter per
/// source, never per tile, so a tile dropped and made again can't take a
/// reply addressed to its earlier self. A tile has at most one job in
/// flight, so `JobId` also names the job.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct JobId {
    pub(crate) tile: TileKey,
    pub(crate) generation: u64,
}

/// Map → worker: the only message.
pub(crate) struct Run {
    pub(crate) id: JobId,
    pub(crate) job: Job,
    /// Set once the tile no longer wants the job's result. Natively, a job
    /// that hasn't started yet then answers `Failed` without running; it
    /// doesn't cross to another instance.
    pub(crate) superseded: Arc<AtomicBool>,
}

/// Worker → map: exactly one per `Run`.
pub(crate) struct Reply {
    pub(crate) id: JobId,
    pub(crate) result: JobResult,
}

#[derive(mln_wire::Wire)]
pub(crate) enum Job {
    Layout(LayoutJob),
    /// Decodes a raster tile's image.
    Raster {
        #[wire(with = "shared_bytes")]
        data: Arc<Vec<u8>>,
    },
    /// Decodes a raster-dem tile's image into its DEM.
    Dem {
        #[wire(with = "shared_bytes")]
        data: Arc<Vec<u8>>,
        encoding: RasterEncoding,
    },
}

/// Everything a layout reads.
#[derive(mln_wire::Wire)]
pub(crate) struct LayoutJob {
    /// The source's style at one version. Crosses once per worker, then by
    /// key; unchanged layers keep their keys.
    #[wire(with = "shared::ByKey::<SharedSourceStyle>")]
    pub(crate) style: Arc<SourceStyle>,
    /// The images that exist, which `image` expressions branch on during
    /// discovery. Crosses by key like the style, but isn't part of its
    /// version: adding an image lays out only the tiles that name it.
    #[wire(with = "shared::ByKey::<SharedAvailableImages>")]
    pub(crate) available_images: Arc<VecSet<String>>,
    pub(crate) data: DataInput,
    pub(crate) params: LayoutParams,
    /// Every resolved dependency the layout may use: empty on a first try,
    /// the previous layout's on a relayout, the answer to `NeedsDeps` on a
    /// resume.
    pub(crate) resolved: Arc<Resolved>,
}

#[derive(mln_wire::Wire)]
pub(crate) enum DataInput {
    /// The worker should still have this data: a hint, which a worker
    /// without it answers with `NeedsData`.
    Cached(DataKey),
    /// The data itself; `None` for a tile without data.
    Full { key: DataKey, data: Option<TileData> },
}

/// One load of a source's data for one canonical tile. `load` comes from a
/// counter of the source that every load takes a new value from, so two
/// loads never share a key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, mln_wire::Wire)]
pub(crate) struct DataKey {
    pub(crate) source: SourceKey,
    pub(crate) tile: CanonicalTileID,
    pub(crate) load: u64,
}

/// What a source's layouts read of the style, at one version.
pub(crate) struct SourceStyle {
    pub(crate) version: u64,
    /// The source's visible layers in style order, for every zoom: a job
    /// lays out those of its tile's zoom.
    pub(crate) layers: Vec<Arc<dyn ErasedEvaluatedLayer>>,
    pub(crate) global_state: Option<Arc<GlobalStateMap>>,
    pub(crate) font_faces: Option<Arc<FontFaces>>,
    /// Whether the style has a `glyphs` URL.
    pub(crate) glyph_pbfs: bool,
}

impl SourceStyle {
    /// Whether this style has these exact values.
    pub(crate) fn is(
        &self,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        global_state: Option<&Arc<GlobalStateMap>>,
        font_faces: Option<&Arc<FontFaces>>,
        glyph_pbfs: bool,
    ) -> bool {
        fn same<T: ?Sized>(a: Option<&Arc<T>>, b: Option<&Arc<T>>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
        }
        self.layers.len() == layers.len()
            && self.layers.iter().zip(layers).all(|(a, b)| Arc::ptr_eq(a, b))
            && same(self.global_state.as_ref(), global_state)
            && same(self.font_faces.as_ref(), font_faces)
            && self.glyph_pbfs == glyph_pbfs
    }
}

#[derive(mln_wire::Wire)]
pub(crate) enum JobResult {
    Laid(LayoutOutput),
    /// The full set this layout needs, of which `resolved` lacked some.
    NeedsDeps(Dependencies),
    /// `Cached(key)` missed: the worker evicted the data or never had it.
    NeedsData,
    /// A raster tile's image.
    Raster(PremultipliedImage),
    /// A raster-dem tile's DEM, with its border filled from its own edges.
    Dem(DEMData),
    Failed(Error),
}

#[derive(mln_wire::Wire)]
pub(crate) struct LayoutOutput {
    /// The buckets and the feature index, which crosses without its tile
    /// data: the tile attaches its own.
    pub(crate) result: LayoutResult,
    /// What the layout used and asked for, unavailable images included, so
    /// that the tile knows which glyph or image changes lay it out again.
    pub(crate) deps: Dependencies,
}

/// How long a job spent in each stage of its layout.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct JobTimes {
    pub(crate) discover: Duration,
    pub(crate) build: Duration,
}

/// Runs `run` and calls `done` with its reply exactly once: before
/// returning, or later on this thread for an image the platform's decoder
/// answers later (the browser's). If the job panics, `done` gets `Failed`
/// as the panic unwinds. `cache` is the worker's tile data cache: `None`
/// natively, where the data always comes with the job. `memo` keeps what a
/// job that answered `NeedsDeps` discovered, for its resume.
pub(crate) fn run_job(
    run: Run,
    cache: Option<&mut DataCache>,
    memo: Option<&DiscoveryMemo>,
    done: impl FnOnce(Reply, JobTimes) + 'static,
) {
    let Run { id, job, .. } = run;
    let guard = ReplyGuard { id, done: Some(done) };
    match job {
        Job::Layout(job) => {
            let (result, times) = layout(id, job, cache, memo);
            guard.finish(result, times);
        }
        // The guard goes with the decode, which replies when it answers.
        Job::Raster { data } => decoder::decode(&data, move |image| {
            let result = image.map_or_else(JobResult::Failed, JobResult::Raster);
            guard.finish(result, JobTimes::default());
        }),
        Job::Dem { data, encoding } => decoder::decode(&data, move |image| {
            let dem = image.and_then(|image| DEMData::new(&image, encoding).map_err(Error::native));
            guard.finish(dem.map_or_else(JobResult::Failed, JobResult::Dem), JobTimes::default());
        }),
    }
}

/// Replies `Failed` if it drops before `finish`: a job that panics still
/// replies, so the transport's count of jobs in flight stays right.
struct ReplyGuard<F: FnOnce(Reply, JobTimes)> {
    id: JobId,
    done: Option<F>,
}

impl<F: FnOnce(Reply, JobTimes)> ReplyGuard<F> {
    fn finish(mut self, result: JobResult, times: JobTimes) {
        if let Some(done) = self.done.take() {
            done(Reply { id: self.id, result }, times);
        }
    }
}

impl<F: FnOnce(Reply, JobTimes)> Drop for ReplyGuard<F> {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            let result = JobResult::Failed(Error::native("the tile's job panicked"));
            done(Reply { id: self.id, result }, JobTimes::default());
        }
    }
}

fn layout(
    id: JobId,
    job: LayoutJob,
    cache: Option<&mut DataCache>,
    memo: Option<&DiscoveryMemo>,
) -> (JobResult, JobTimes) {
    let LayoutJob { style, available_images, data, params, resolved } = job;
    let mut times = JobTimes::default();
    let data = match (data, cache) {
        (DataInput::Full { key, data }, Some(cache)) => {
            if let Some(data) = &data {
                cache.insert(key, data.clone());
            }
            data
        }
        (DataInput::Full { data, .. }, None) => data,
        (DataInput::Cached(key), cache) => match cache.and_then(|cache| cache.get(&key)) {
            Some(data) => Some(data),
            None => return (JobResult::NeedsData, times),
        },
    };

    let tile_id = id.tile.id;
    let zoom = f32::from(tile_id.overscaled_z);
    let layers: Vec<Arc<dyn ErasedEvaluatedLayer>> = style
        .layers
        .iter()
        .filter(|layer| zoom >= layer.style().base().min_zoom.floor() && zoom < layer.style().base().max_zoom.ceil())
        .cloned()
        .collect();
    let input = LayoutInput {
        id: tile_id,
        data: data.as_ref(),
        layers: &layers,
        global_state: style.global_state.as_ref(),
        font_faces: style.font_faces.as_ref(),
        glyph_pbfs: style.glyph_pbfs,
        available_images: &available_images,
        params,
    };

    // A resume of the same generation discovers what its first try did.
    let memoized = memo.and_then(|memo| memo.take(&id, &style, &available_images));
    let (discovered, deps) = if let Some(memoized) = memoized {
        memoized
    } else {
        let start = Instant::now();
        let discovered = discover(&input);
        times.discover = start.elapsed();
        match discovered {
            Ok(mut discovered) => {
                let deps = std::mem::take(&mut discovered.dependencies);
                (discovered, deps)
            }
            Err(error) => return (JobResult::Failed(error), times),
        }
    };
    if !deps.satisfied_by(&resolved) {
        if let Some(memo) = memo {
            memo.keep(id, Memoized { style, available_images, discovered, deps: deps.clone() });
        }
        return (JobResult::NeedsDeps(deps), times);
    }

    let start = Instant::now();
    let built = build(discovered, &input, &resolved);
    times.build = start.elapsed();
    let result = match built {
        Ok(built) => JobResult::Laid(LayoutOutput {
            result: LayoutResult::new(built.buckets, Arc::new(built.feature_index.finish())),
            deps,
        }),
        Err(error) => JobResult::Failed(error),
    };
    (result, times)
}

/// What a job that answered `NeedsDeps` discovered, kept for its resume: a
/// cache, whose miss discovers again. A resume is a job of the same
/// generation, so of the same inputs but `resolved`. It keeps one entry per
/// tile, which the tile's next job takes, resume or not.
pub(crate) struct DiscoveryMemo {
    capacity: usize,
    /// Oldest first.
    entries: Mutex<VecDeque<(JobId, Memoized)>>,
}

struct Memoized {
    /// The inputs whose addresses `discovered` keys its filter groups by,
    /// kept alive so that no other value takes their place.
    style: Arc<SourceStyle>,
    available_images: Arc<VecSet<String>>,
    discovered: Discovered,
    deps: Dependencies,
}

impl DiscoveryMemo {
    /// A memo of the last `capacity` jobs that answered `NeedsDeps`.
    pub(crate) fn new(capacity: usize) -> Self {
        DiscoveryMemo { capacity, entries: Mutex::new(VecDeque::new()) }
    }

    /// Takes the entry of job `id`'s tile: what the job of the same
    /// generation discovered, for a resume with the same style and images.
    /// Another generation's entry goes, since no job will take it.
    fn take(
        &self,
        id: &JobId,
        style: &Arc<SourceStyle>,
        available_images: &Arc<VecSet<String>>,
    ) -> Option<(Discovered, Dependencies)> {
        let (kept, memoized) = {
            let mut entries = self.entries.lock();
            let index = entries.iter().position(|(kept, _)| kept.tile == id.tile)?;
            entries.remove(index)?
        };
        let same = kept == *id
            && Arc::ptr_eq(&memoized.style, style)
            && Arc::ptr_eq(&memoized.available_images, available_images);
        same.then_some((memoized.discovered, memoized.deps))
    }

    fn keep(&self, id: JobId, memoized: Memoized) {
        let evicted = {
            let mut entries = self.entries.lock();
            entries.push_back((id, memoized));
            let excess = entries.len().saturating_sub(self.capacity);
            entries.drain(..excess).collect::<Vec<_>>()
        };
        // Freed outside the lock.
        drop(evicted);
    }
}

/// What a worker's data cache keeps by default.
pub(crate) const DEFAULT_DATA_CACHE_BYTES: usize = 32 << 20;

/// A worker's tile data, by `DataKey`, up to a budget of bytes: the least
/// recently used goes first. A tile without data has nothing to keep: its
/// jobs always say so.
pub(crate) struct DataCache {
    budget: usize,
    retained: usize,
    entries: HashMap<DataKey, CachedData>,
    /// Counts the uses, for the recency of each entry.
    clock: u64,
}

struct CachedData {
    data: TileData,
    bytes: usize,
    used: u64,
}

impl DataCache {
    pub(crate) fn new(budget: usize) -> Self {
        DataCache { budget, retained: 0, entries: HashMap::default(), clock: 0 }
    }

    /// The bytes the cache keeps, by its estimate.
    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained
    }

    pub(crate) fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.evict(None);
    }

    fn get(&mut self, key: &DataKey) -> Option<TileData> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.used = self.clock;
        Some(entry.data.clone())
    }

    /// Keeps `data`, then evicts until the cache fits its budget again; it
    /// keeps the data just added even if that alone is over.
    fn insert(&mut self, key: DataKey, data: TileData) {
        self.clock += 1;
        let bytes = data_bytes(&data);
        let entry = CachedData { data, bytes, used: self.clock };
        if let Some(previous) = self.entries.insert(key, entry) {
            self.retained -= previous.bytes;
        }
        self.retained += bytes;
        self.evict(Some(key));
    }

    fn evict(&mut self, keep: Option<DataKey>) {
        while self.retained > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter(|(key, _)| Some(**key) != keep)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| *key);
            let Some(entry) = oldest.and_then(|key| self.entries.remove(&key)) else {
                break;
            };
            self.retained -= entry.bytes;
        }
    }
}

/// About how many bytes `data` takes: its encoded bytes, or for features
/// their structs and coordinates.
fn data_bytes(data: &TileData) -> usize {
    match data {
        TileData::Mvt(tile) => tile.bytes().len(),
        TileData::Mlt(tile) => tile.bytes().len(),
        TileData::Features(features) => features
            .iter()
            .map(|feature| {
                let coordinates = feature.geometry.as_ref().map_or(0, coordinate_count);
                size_of_val(feature) + coordinates * 4 + feature.properties.len() * 64
            })
            .sum(),
        // The ancestor stays while the map side holds it, whatever the
        // cache keeps: a cut tile's own data is about its share of it.
        TileData::Cut(data) => {
            data_bytes(data.ancestor().data()).checked_shr(2 * u32::from(data.cut().dz)).unwrap_or(0)
        }
    }
}

fn coordinate_count(geometry: &geo_types::Geometry<i16>) -> usize {
    use geo_types::Geometry;
    let polygon = |polygon: &geo_types::Polygon<i16>| {
        polygon.exterior().0.len() + polygon.interiors().iter().map(|ring| ring.0.len()).sum::<usize>()
    };
    match geometry {
        Geometry::Point(_) => 1,
        Geometry::Line(_) => 2,
        Geometry::LineString(line) => line.0.len(),
        Geometry::Polygon(shape) => polygon(shape),
        Geometry::MultiPoint(points) => points.0.len(),
        Geometry::MultiLineString(lines) => lines.0.iter().map(|line| line.0.len()).sum(),
        Geometry::MultiPolygon(polygons) => polygons.0.iter().map(polygon).sum(),
        Geometry::GeometryCollection(geometries) => geometries.0.iter().map(coordinate_count).sum(),
        Geometry::Rect(_) | Geometry::Triangle(_) => 4,
    }
}
