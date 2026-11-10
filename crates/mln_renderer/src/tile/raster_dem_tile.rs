// Ports src/mln/tile/raster_dem_tile.hpp, src/mln/tile/raster_dem_tile.cpp,
// src/mln/tile/raster_dem_tile_worker.hpp and
// src/mln/tile/raster_dem_tile_worker.cpp.
//
// Same structure as raster_tile.rs: the state the worker's replies change
// is `RasterDEMTileInner`, the target of the tile's mailbox, and the tile
// and its worker exchange only `RasterDEMWorkerMessage`s and
// `RasterDEMTileMessage`s through the `TileWorkerTransport` of the
// TileParameters (tile_worker_transport.rs).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_platform_default_image::decoder;
use mln_storage::Data;
use mln_style::layer_properties::LayerProperties;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::exception::{ExceptionPtr, RuntimeError};
use mln_util::immutable::Immutable;
use mln_util::serialization::payload::shared_bytes;
use mln_util::tileset::{RasterEncoding, Tileset};
use serde::{Deserialize, Serialize};

use crate::bucket::BucketPtr;
use crate::buckets::hillshade_bucket::HillshadeBucket;
use crate::tile::ordered_replies::OrderedReplies;
use crate::tile::tile::{Kind, Tile, TileBase, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_loader::{LoaderTile, TileLoader, VectorTileEncoding};
use crate::tile::tile_observer::TileObserver;
use crate::tile::tile_worker_transport::{TileEndpoint, WorkerEndpoint};
use crate::tile::worker_codec::fields;
use crate::tile_mask::TileMask;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::{SharedBucketTileRenderData, TileRenderData};

/// `DEMTileNeighbors`: a bitmask of the backfilled neighbors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DEMTileNeighbors(pub u8);

impl DEMTileNeighbors {
    pub const EMPTY: DEMTileNeighbors = DEMTileNeighbors(0);
    pub const LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 0);
    pub const RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 1);
    pub const TOP_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 2);
    pub const TOP_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 3);
    pub const TOP_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 4);
    pub const BOTTOM_LEFT: DEMTileNeighbors = DEMTileNeighbors(1 << 5);
    pub const BOTTOM_CENTER: DEMTileNeighbors = DEMTileNeighbors(1 << 6);
    pub const BOTTOM_RIGHT: DEMTileNeighbors = DEMTileNeighbors(1 << 7);

    // helper enums for tiles with no upper/lower neighbors and completely
    // backfilled tiles
    pub const NO_UPPER: DEMTileNeighbors = DEMTileNeighbors(0b0001_1100);
    pub const NO_LOWER: DEMTileNeighbors = DEMTileNeighbors(0b1110_0000);
    pub const COMPLETE: DEMTileNeighbors = DEMTileNeighbors(0b1111_1111);
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

/// The messages a `RasterDEMTile` sends its `RasterDEMTileWorker`: the
/// worker's public member functions. Port-only: serde writes them for a
/// worker in another wasm instance (tile/worker_codec).
#[derive(Serialize, Deserialize)]
pub enum RasterDEMWorkerMessage {
    /// `parse(data, correlationID, encoding)`.
    Parse {
        #[serde(with = "shared_bytes::option")]
        data: Option<Data>,
        correlation_id: u64,
        encoding: RasterEncoding,
    },
}

/// The messages a `RasterDEMTileWorker` sends its `RasterDEMTile`: the
/// tile's member functions the worker calls. Port-only: serde writes them
/// for a worker in another wasm instance (tile/worker_codec).
#[derive(Serialize, Deserialize)]
pub enum RasterDEMTileMessage {
    /// `onParsed(std::move(bucket), correlationID)`.
    Parsed { bucket: Option<Arc<HillshadeBucket>>, correlation_id: u64 },
    /// `onError(std::current_exception(), correlationID)`.
    Error {
        #[serde(with = "fields::exception")]
        error: ExceptionPtr,
        correlation_id: u64,
    },
}

/// `RasterDEMTileWorker`.
pub struct RasterDEMTileWorker {
    /// The tile, which gets the replies in the order of their parses
    /// although images may decode asynchronously (ordered_replies.rs).
    parent: OrderedReplies<RasterDEMTileMessage>,
}

impl RasterDEMTileWorker {
    /// `RasterDEMTileWorker(ActorRef<RasterDEMTileWorker>, ActorRef<RasterDEMTile>)`.
    pub fn new(parent: Box<dyn TileEndpoint<RasterDEMTileMessage>>) -> Self {
        RasterDEMTileWorker { parent: OrderedReplies::new(parent) }
    }

    /// Runs the member function a tile's message stands for.
    pub fn handle(&mut self, message: RasterDEMWorkerMessage) {
        match message {
            RasterDEMWorkerMessage::Parse { data, correlation_id, encoding } => {
                self.parse(data, correlation_id, encoding)
            }
        }
    }

    /// `parse(data, correlationID, encoding)`. The reply goes once the
    /// image has decoded, which may be after this returns (decoder.rs in
    /// mln_platform_default_image).
    pub fn parse(&mut self, data: Option<Data>, correlation_id: u64, encoding: RasterEncoding) {
        let parent = self.parent.slot();
        let Some(data) = data else {
            // No data; empty tile.
            parent.send(RasterDEMTileMessage::Parsed { bucket: None, correlation_id });
            return;
        };

        decoder::decode(&data, move |result| match result.and_then(|image| HillshadeBucket::new(image, encoding)) {
            Ok(bucket) => {
                parent.send(RasterDEMTileMessage::Parsed { bucket: Some(Arc::new(bucket)), correlation_id });
            }
            Err(error) => {
                parent.send(RasterDEMTileMessage::Error { error: RuntimeError::ptr(error), correlation_id });
            }
        });
    }
}

/// The render-thread state of a `RasterDEMTile`.
pub struct RasterDEMTileInner {
    pub base: TileBase,
    obsolete: Cell<bool>,
    correlation_id: Cell<u64>,
    encoding: RasterEncoding,
    /// Contains the Bucket object for the tile. Buckets are render objects
    /// and they get added by tile parsing operations.
    bucket: RefCell<Option<Arc<HillshadeBucket>>>,
    /// neighboringTiles is a bitmask for which neighboring tiles have been
    /// backfilled; there are max 8 possible neighboring tiles, so each bit
    /// represents one neighbor
    pub neighboring_tiles: Cell<DEMTileNeighbors>,
    /// `Actor<RasterDEMTileWorker> worker`, behind its transport.
    worker: Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>>,
    mailbox: Arc<Mailbox>,
}

impl RasterDEMTileInner {
    /// Runs the member function a worker's message stands for.
    pub fn handle_worker_message(&self, message: RasterDEMTileMessage) {
        match message {
            RasterDEMTileMessage::Parsed { bucket, correlation_id } => self.on_parsed(bucket, correlation_id),
            RasterDEMTileMessage::Error { error, correlation_id } => self.on_error(error, correlation_id),
        }
    }

    /// `onParsed(std::unique_ptr<HillshadeBucket>, correlationID)`.
    pub fn on_parsed(&self, result: Option<Arc<HillshadeBucket>>, result_correlation_id: u64) {
        if !self.obsolete.get() {
            let renderable = result.is_some();
            *self.bucket.borrow_mut() = result;
            self.base.loaded.set(true);
            if result_correlation_id == self.correlation_id.get() {
                self.base.pending.set(false);
                self.base.on_tile_action(TileOperation::EndParse);
            }
            self.base.renderable.set(renderable);
            self.base.observer().on_tile_changed(&self.base);
        }
    }

    /// `onError(std::exception_ptr, correlationID)`.
    pub fn on_error(&self, err: ExceptionPtr, result_correlation_id: u64) {
        self.base.loaded.set(true);
        if result_correlation_id == self.correlation_id.get() {
            self.base.pending.set(false);
            self.base.on_tile_action(TileOperation::Error);
        }
        self.base.observer().on_tile_error(&self.base, err);
    }

    /// `markObsolete()`.
    fn mark_obsolete(&self) {
        self.obsolete.set(true);
        if self.base.pending.get() {
            self.base.on_tile_action(TileOperation::Cancelled);
        }
        self.base.pending.set(false);
        // `mailbox->abandon()`
        self.mailbox.close();
    }
}

impl LoaderTile for RasterDEMTileInner {
    /// `setError(std::exception_ptr)`.
    fn set_error(&self, err: ExceptionPtr) {
        self.base.loaded.set(true);
        self.base.observer().on_tile_error(&self.base, err);
    }

    fn set_tried_cache(&self) {
        self.base.set_tried_cache();
    }

    fn on_tile_action(&self, op: TileOperation) {
        self.base.on_tile_action(op);
    }

    /// `setMetadata(modified, expires)`.
    fn set_metadata(&self, modified: Option<Timestamp>, expires: Option<Timestamp>) {
        self.base.modified.set(modified);
        self.base.expires.set(expires);
    }

    /// `setData(data)`.
    fn set_data(&self, data: Option<Data>, _encoding: VectorTileEncoding) {
        if !self.obsolete.get() {
            self.correlation_id.set(self.correlation_id.get() + 1);
            if !self.base.pending.get() {
                self.base.on_tile_action(TileOperation::StartParse);
            }
            self.base.pending.set(true);
            let correlation_id = self.correlation_id.get();
            let encoding = self.encoding;
            self.worker.send(RasterDEMWorkerMessage::Parse { data, correlation_id, encoding });
        }
    }
}

/// `RasterDEMTile`.
pub struct RasterDEMTile {
    inner: Rc<RasterDEMTileInner>,
    loader: Option<TileLoader>,
}

impl RasterDEMTile {
    /// `RasterDEMTile(id, sourceID, parameters, tileset, observer)`.
    pub fn new(
        id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        tileset: &Tileset,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let mailbox = Mailbox::new();
        let parent: ActorRef<RasterDEMTileInner> = ActorRef::new(&mailbox);
        let worker = parameters.tile_worker_transport.create_raster_dem(Box::new(parent));

        let encoding = tileset.raster_encoding.unwrap_or(RasterEncoding::Mapbox);
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

        let inner = Rc::new(RasterDEMTileInner {
            base: TileBase::new(Kind::RasterDEM, id, source_id, observer),
            obsolete: Cell::new(false),
            correlation_id: Cell::new(0),
            encoding,
            bucket: RefCell::new(None),
            neighboring_tiles: Cell::new(neighboring_tiles),
            worker,
            mailbox: mailbox.clone(),
        });
        mailbox.open(&scheduler::get_current(), inner.clone() as Rc<dyn Any>);
        let weak: std::rc::Weak<RasterDEMTileInner> = Rc::downgrade(&inner);
        let loader = TileLoader::new(weak, VectorTileEncoding::Mvt, &id, parameters, tileset);
        RasterDEMTile { inner, loader: Some(loader) }
    }

    pub fn inner(&self) -> &Rc<RasterDEMTileInner> {
        &self.inner
    }

    /// `getBucket()`.
    pub fn get_bucket(&self) -> Option<Arc<HillshadeBucket>> {
        self.inner.bucket.borrow().clone()
    }

    /// `neighboringTiles`.
    pub fn neighboring_tiles(&self) -> DEMTileNeighbors {
        self.inner.neighboring_tiles.get()
    }

    /// `backfillBorder(const RasterDEMTile& borderTile, DEMTileNeighbors mask)`.
    pub fn backfill_border(&self, border_tile: &RasterDEMTile, mask: DEMTileNeighbors) {
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

impl Drop for RasterDEMTile {
    fn drop(&mut self) {
        self.loader = None;
        self.inner.mark_obsolete();
        if self.inner.base.pending.get() {
            // This tile never finished loading or was abandoned, emit a
            // cancellation event
            self.inner.base.on_tile_action(TileOperation::Cancelled);
        }
    }
}

impl Tile for RasterDEMTile {
    fn base(&self) -> &TileBase {
        &self.inner.base
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        let bucket: Option<BucketPtr> = self.inner.bucket.borrow().clone().map(|b| b as BucketPtr);
        Box::new(SharedBucketTileRenderData::new(bucket))
    }

    fn set_necessity(&self, necessity: TileNecessity) {
        if let Some(loader) = &self.loader {
            loader.set_necessity(necessity);
        }
    }

    fn set_update_parameters(&self, parameters: &TileUpdateParameters) {
        if let Some(loader) = &self.loader {
            loader.set_update_parameters(parameters);
        }
    }

    fn cancel(&self) {
        self.inner.mark_obsolete();
    }

    fn layer_properties_updated(&self, _layer_properties: &Immutable<dyn LayerProperties>) -> bool {
        self.inner.bucket.borrow().is_some()
    }

    fn set_mask(&self, mask: TileMask) {
        if let Some(bucket) = &*self.inner.bucket.borrow() {
            bucket.set_mask(mask);
        }
    }
}
