// A raster or raster-dem tile decodes its data in one job (tile/job.rs):
// `Job::Raster` into an image, `Job::Dem` into a DEM. Each change of its data
// takes a new generation from its source's counters, and the tile applies a
// reply only if it names the current generation, so replies may come in any
// order. The tile keeps its data until the decode of the current generation
// replies, so a job whose lane is lost runs again.
//
// The renderer's update drives the tiles (`take_reply` before the sources
// update, `drive` after), and a tile submits its job itself as soon as its
// data arrives if the renderer's jobs are free.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_storage::Data;
use mln_style::layer_properties::LayerProperties;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::error::Error;
use mln_util::tileset::{RasterEncoding, Tileset};

use crate::bucket::BucketPtr;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::buckets::raster_bucket::RasterBucket;
use crate::tile::geometry_tile::DependencyStores;
use crate::tile::job::{Job, JobId, JobResult, Run, TileKey};
use crate::tile::tile::{Kind, Tile, TileBase, TileLoadOrder, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_loader::{LoaderTile, TileLoader, VectorTileEncoding};
use crate::tile::tile_observer::TileObserver;
use crate::tile::tile_workers::{Lane, SourceCounters, TileJobs};
use crate::tile_mask::TileMask;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::{SharedBucketTileRenderData, TileRenderData};

/// A bitmask of the neighbors backfilled into the tile's DEM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DEMTileNeighbors(pub(crate) u8);

impl DEMTileNeighbors {
    pub(crate) const EMPTY: DEMTileNeighbors = DEMTileNeighbors(0);
    pub(crate) const LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 0);
    pub(crate) const RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 1);
    pub(crate) const TOP_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 2);
    pub(crate) const TOP_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 3);
    pub(crate) const TOP_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 4);
    pub(crate) const BOTTOM_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 5);
    pub(crate) const BOTTOM_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 6);
    pub(crate) const BOTTOM_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 7);

    // helper enums for tiles with no upper/lower neighbors and completely
    // backfilled tiles
    pub(crate) const NO_UPPER: DEMTileNeighbors = DEMTileNeighbors(0b0001_1100);
    pub(crate) const NO_LOWER: DEMTileNeighbors = DEMTileNeighbors(0b1110_0000);
    pub(crate) const COMPLETE: DEMTileNeighbors = DEMTileNeighbors(0b1111_1111);
}

impl std::ops::BitOr for DEMTileNeighbors {
    type Output = DEMTileNeighbors;
    fn bitor(self, rhs: Self) -> Self {
        DEMTileNeighbors(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for DEMTileNeighbors {
    type Output = DEMTileNeighbors;
    fn bitand(self, rhs: Self) -> Self {
        DEMTileNeighbors(self.0 & rhs.0)
    }
}

/// What a raster tile decodes its data into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RasterKind {
    /// An image, which a raster layer draws.
    Image,
    /// A DEM of this encoding, which hillshade and color-relief layers draw.
    Dem(RasterEncoding),
}

/// What a raster tile draws: the bucket of its decoded image or DEM.
#[derive(Clone)]
enum Decoded {
    Image(Arc<RasterBucket>),
    Dem(Arc<HillshadeBucket>),
}

impl Decoded {
    fn bucket(&self) -> BucketPtr {
        match self {
            Decoded::Image(bucket) => Arc::<RasterBucket>::clone(bucket),
            Decoded::Dem(bucket) => Arc::<HillshadeBucket>::clone(bucket),
        }
    }

    fn set_mask(&self, mask: TileMask) {
        match self {
            Decoded::Image(bucket) => bucket.set_mask(mask),
            Decoded::Dem(bucket) => bucket.set_mask(mask),
        }
    }
}

/// The tile's job in flight, of the current generation or an earlier one.
struct Running {
    id: JobId,
    lane: Lane,
    superseded: Arc<AtomicBool>,
}

/// A tile's side of its jobs.
struct Driver {
    generation: u64,
    /// The data to decode, until the job of the current generation replies.
    data: Option<Data>,
    running: Option<Running>,
}

impl Driver {
    /// Takes a new generation: a job in flight becomes stale.
    fn bump(&mut self, source: &SourceCounters) {
        self.generation = source.next_generation();
        if let Some(running) = &self.running {
            running.superseded.store(true, Ordering::Relaxed);
        }
    }
}

/// The render-thread state of a `RasterTile`.
pub(crate) struct RasterTileInner {
    pub(crate) base: TileBase,
    kind: RasterKind,
    source: Rc<SourceCounters>,
    /// Where the tile submits its job as soon as its data arrives, outside
    /// the renderer's update.
    jobs: Option<Rc<RefCell<TileJobs>>>,
    /// Set once the tile is cancelled or dropped: it takes no more data.
    obsolete: Cell<bool>,
    driver: RefCell<Driver>,
    decoded: RefCell<Option<Decoded>>,
    /// raster-dem: the neighbours whose borders the DEM has (a bit each).
    neighboring_tiles: Cell<DEMTileNeighbors>,
}

impl RasterTileInner {
    fn key(&self) -> TileKey {
        TileKey { source: self.source.key(), id: self.base.id() }
    }

    /// What the tile shows.
    fn decoded(&self) -> Option<Decoded> {
        self.decoded.borrow().clone()
    }

    /// Takes in a load: the data to decode, or for a tile without data,
    /// nothing to draw, which leaves the tile renderable unless the tileset
    /// is `missing` it.
    fn load(&self, data: Option<Data>, missing: bool) {
        if self.obsolete.get() {
            return;
        }
        if !self.base.pending.get() {
            self.base.on_tile_action(TileOperation::StartParse);
        }
        self.base.pending.set(true);
        let has_data = data.is_some();
        {
            let mut driver = self.driver.borrow_mut();
            driver.bump(&self.source);
            driver.data = data;
        }
        if has_data {
            self.advance();
        } else {
            self.show(None, !missing);
        }
    }

    /// Starts the tile's job now if the renderer's jobs are free and a lane
    /// has room; otherwise asks for the update that starts it.
    fn advance(&self) {
        if let Some(jobs) = &self.jobs
            && let Ok(mut jobs) = jobs.try_borrow_mut()
        {
            self.submit(&mut jobs);
        }
        // A job in flight wakes the update when it replies.
        if self.has_work() {
            self.base.observer().on_update_needed(&self.base);
        }
    }

    /// Whether the tile has a job to submit.
    fn has_work(&self) -> bool {
        let driver = self.driver.borrow();
        !self.obsolete.get() && driver.running.is_none() && driver.data.is_some()
    }

    /// Submits the tile's job to `jobs`, if it has one and a lane has room.
    fn submit(&self, jobs: &mut TileJobs) {
        if !self.has_work() {
            return;
        }
        let Some(lane) = jobs.lane() else {
            return;
        };
        let mut driver = self.driver.borrow_mut();
        let Some(data) = driver.data.clone() else {
            return;
        };
        let job = match self.kind {
            RasterKind::Image => Job::Raster { data },
            RasterKind::Dem(encoding) => Job::Dem { data, encoding },
        };
        let id = JobId { tile: self.key(), generation: driver.generation };
        let superseded = Arc::new(AtomicBool::new(false));
        jobs.submit(lane, Run { id, job, superseded: Arc::clone(&superseded) });
        driver.running = Some(Running { id, lane, superseded });
    }

    /// Takes the reply to the tile's job, if it arrived, and gives up the
    /// job if its lane was lost. A stale reply stays with `jobs`, undecoded.
    /// Returns whether the tile shows a new image or DEM.
    fn take_reply(&self, jobs: &mut TileJobs) -> bool {
        let mut driver = self.driver.borrow_mut();
        let Some(Running { id, lane, .. }) = driver.running else {
            return false;
        };
        if !jobs.has_reply(&id) {
            if jobs.is_lost(lane) {
                // Its reply never comes: the data goes out again.
                driver.running = None;
                driver.bump(&self.source);
                drop(driver);
                self.base.observer().on_update_needed(&self.base);
            }
            return false;
        }
        driver.running = None;
        if id.generation != driver.generation {
            drop(driver);
            // The tile decodes its newest data instead.
            if self.has_work() {
                self.base.observer().on_update_needed(&self.base);
            }
            return false;
        }
        driver.data = None;
        drop(driver);
        jobs.take_reply(&id).is_some_and(|result| self.apply(result))
    }

    /// Applies the result of the tile's current job. Returns whether the
    /// tile shows a new image or DEM.
    pub(crate) fn apply(&self, result: JobResult) -> bool {
        let decoded = match result {
            JobResult::Raster(image) => Decoded::Image(Arc::new(RasterBucket::new(image))),
            JobResult::Dem(dem) => Decoded::Dem(Arc::new(HillshadeBucket::with_dem_data(dem))),
            JobResult::Failed(error) => {
                self.on_error(error);
                return false;
            }
            JobResult::Laid(_) | JobResult::NeedsDeps(_) | JobResult::NeedsData => {
                self.on_error(Error::native("a raster tile's decode answered as a layout"));
                return false;
            }
        };
        self.show(Some(decoded), true);
        true
    }

    /// Shows `decoded`, or nothing; the tile is renderable if it shows
    /// something or `renderable` says so.
    fn show(&self, decoded: Option<Decoded>, renderable: bool) {
        let renderable = decoded.is_some() || renderable;
        *self.decoded.borrow_mut() = decoded;
        self.base.loaded.set(true);
        self.base.pending.set(false);
        self.base.on_tile_action(TileOperation::EndParse);
        self.base.renderable.set(renderable);
        self.base.observer().on_tile_changed(&self.base);
    }

    /// The decode of the tile's data failed: the tile keeps what it showed.
    pub(crate) fn on_error(&self, err: Error) {
        self.base.loaded.set(true);
        self.base.pending.set(false);
        self.base.on_tile_action(TileOperation::Error);
        self.base.observer().on_tile_error(&self.base, err);
    }

    /// Lets go of the tile's job: it takes no more data, and a job not
    /// started yet needn't run.
    fn mark_obsolete(&self) {
        self.obsolete.set(true);
        if self.base.pending.get() {
            self.base.on_tile_action(TileOperation::Cancelled);
        }
        self.base.pending.set(false);
        let mut driver = self.driver.borrow_mut();
        driver.bump(&self.source);
        driver.data = None;
    }
}

impl LoaderTile for RasterTileInner {
    fn set_error(&self, err: Error) {
        self.base.loaded.set(true);
        self.base.observer().on_tile_error(&self.base, err);
    }

    fn set_loading(&self) {
        self.base.loaded.set(false);
    }

    fn set_tried_cache(&self) {
        self.base.set_tried_cache();
    }

    fn on_tile_action(&self, op: TileOperation) {
        self.base.on_tile_action(op);
    }

    fn set_metadata(&self, modified: Option<Timestamp>, expires: Option<Timestamp>) {
        self.base.modified.set(modified);
        self.base.expires.set(expires);
    }

    fn set_data(&self, data: Option<Data>, _encoding: VectorTileEncoding) {
        self.load(data, false);
    }

    fn set_missing(&self, _encoding: VectorTileEncoding) {
        self.load(None, true);
    }
}

/// A raster or raster-dem tile.
pub(crate) struct RasterTile {
    inner: Rc<RasterTileInner>,
    loader: Option<TileLoader>,
}

impl RasterTile {
    #[must_use]
    pub(crate) fn new(
        id: OverscaledTileID,
        source_id: String,
        kind: RasterKind,
        parameters: &TileParameters<'_>,
        tileset: &Tileset,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let mut neighboring_tiles = DEMTileNeighbors::EMPTY;
        if id.canonical.y == 0 {
            // this tile doesn't have upper neighboring tiles so marked those
            // as backfilled
            neighboring_tiles = neighboring_tiles | DEMTileNeighbors::NO_UPPER;
        }

        if f64::from(id.canonical.y + 1) == 2f64.powi(i32::from(id.canonical.z)) {
            // this tile doesn't have lower neighboring tiles so marked those
            // as backfilled
            neighboring_tiles = neighboring_tiles | DEMTileNeighbors::NO_LOWER;
        }

        let tile_kind = match kind {
            RasterKind::Image => Kind::Raster,
            RasterKind::Dem(_) => Kind::RasterDEM,
        };
        let source = Rc::clone(&parameters.source);
        let driver = Driver { generation: source.next_generation(), data: None, running: None };
        let inner = Rc::new(RasterTileInner {
            base: TileBase::new(tile_kind, id, source_id, observer),
            kind,
            source,
            jobs: parameters.jobs.clone(),
            obsolete: Cell::new(false),
            driver: RefCell::new(driver),
            decoded: RefCell::new(None),
            neighboring_tiles: Cell::new(neighboring_tiles),
        });
        let weak: std::rc::Weak<RasterTileInner> = Rc::downgrade(&inner);
        let loader = TileLoader::new(weak, VectorTileEncoding::Mvt, &id, parameters, tileset);
        RasterTile { inner, loader: Some(loader) }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn inner(&self) -> &Rc<RasterTileInner> {
        &self.inner
    }

    /// The bucket of the tile's DEM, for a raster-dem tile that has one.
    #[must_use]
    pub(crate) fn get_bucket(&self) -> Option<Arc<HillshadeBucket>> {
        match self.inner.decoded()? {
            Decoded::Dem(bucket) => Some(bucket),
            Decoded::Image(_) => None,
        }
    }

    #[must_use]
    pub(crate) fn neighboring_tiles(&self) -> DEMTileNeighbors {
        self.inner.neighboring_tiles.get()
    }

    pub(crate) fn backfill_border(&self, border_tile: &RasterTile, mask: DEMTileNeighbors) {
        let id = self.inner.base.id();
        let border_id = border_tile.inner.base.id();
        let mut dx: i32 = border_id.canonical.x as i32 - id.canonical.x as i32;
        let dy: i8 = (border_id.canonical.y as i32 - id.canonical.y as i32) as i8;
        let dim = 2f64.powi(i32::from(id.canonical.z)) as u32;
        if dx == 0 && dy == 0 {
            return;
        }
        if dy.unsigned_abs() > 1 {
            return;
        }
        // neighbor is in another world wrap
        if dx.abs() > 1 {
            if (dx + dim as i32).abs() == 1 {
                dx += dim as i32;
            } else if (dx - dim as i32).abs() == 1 {
                dx -= dim as i32;
            }
        }
        let Some(border_bucket) = border_tile.get_bucket() else {
            return;
        };
        let Some(bucket) = self.get_bucket() else {
            return;
        };
        if Arc::ptr_eq(&border_bucket, &bucket) {
            return;
        }
        {
            let border_dem = border_bucket.get_dem_data();
            let mut tile_dem = bucket.get_dem_data_mut();
            tile_dem.backfill_border(&border_dem, dx as i8, dy);
        }
        // update the bitmask to indicate that this tiles have been
        // backfilled by flipping the relevant bit
        self.inner.neighboring_tiles.set(self.inner.neighboring_tiles.get() | mask);
        // mark HillshadeBucket.prepared as false so it runs through the
        // prepare render pass with the new texture data we just backfilled
        bucket.set_prepared(false);
        bucket.reset_render_target_prepared();
    }
}

impl Drop for RasterTile {
    fn drop(&mut self) {
        self.loader = None;
        self.inner.mark_obsolete();
    }
}

impl Tile for RasterTile {
    fn base(&self) -> &TileBase {
        &self.inner.base
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        let bucket = self.inner.decoded().as_ref().map(Decoded::bucket);
        Box::new(SharedBucketTileRenderData::new(bucket))
    }

    fn set_necessity(&self, necessity: TileNecessity) {
        if let Some(loader) = &self.loader {
            loader.set_necessity(necessity);
        }
    }

    fn set_load_order(&self, order: TileLoadOrder) {
        if let Some(loader) = &self.loader {
            loader.set_load_order(order);
        }
    }

    fn set_update_parameters(&self, parameters: &TileUpdateParameters) {
        if let Some(loader) = &self.loader {
            loader.set_update_parameters(parameters);
        }
    }

    fn retry_failed_load(&self) {
        if let Some(loader) = &self.loader {
            loader.retry_failed_load();
        }
    }

    fn cancel(&self) {
        self.inner.mark_obsolete();
    }

    fn take_reply(&self, jobs: &mut TileJobs) -> bool {
        self.inner.take_reply(jobs)
    }

    fn has_work(&self) -> bool {
        self.inner.has_work()
    }

    fn drive(&self, jobs: &mut TileJobs, _stores: &mut DependencyStores<'_>) -> bool {
        self.inner.submit(jobs);
        false
    }

    fn layer_properties_updated(&self, _layer_properties: &Arc<dyn LayerProperties>) -> bool {
        self.inner.decoded().is_some()
    }

    fn set_mask(&self, mask: TileMask) {
        if let Some(decoded) = self.inner.decoded() {
            decoded.set_mask(mask);
        }
    }
}
