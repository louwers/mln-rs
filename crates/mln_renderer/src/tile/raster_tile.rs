// Ports src/mln/tile/raster_tile.hpp, src/mln/tile/raster_tile.cpp,
// src/mln/tile/raster_tile_worker.hpp and src/mln/tile/raster_tile_worker.cpp.
//
// As for GeometryTile (see geometry_tile.rs), the state the worker's
// replies change is `RasterTileInner`, the target of the tile's mailbox.
// The tile and its worker exchange only `RasterWorkerMessage`s and
// `RasterTileMessage`s, and the worker is created by the
// `TileWorkerTransport` of the TileParameters (tile_worker_transport.rs).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{ActorRef, Mailbox, scheduler};
use mln_image::decoder;
use mln_storage::Data;
use mln_style::layer_properties::LayerProperties;
use mln_tile_id::OverscaledTileID;
use mln_util::chrono::Timestamp;
use mln_util::error::Error;
use mln_util::immutable::Immutable;
use mln_util::serialization::payload::shared_bytes;
use mln_util::tileset::Tileset;
use serde::{Deserialize, Serialize};

use crate::bucket::BucketPtr;
use crate::buckets::raster_bucket::RasterBucket;
use crate::tile::ordered_replies::OrderedReplies;
use crate::tile::tile::{Kind, Tile, TileBase, TileNecessity, TileOperation, TileUpdateParameters};
use crate::tile::tile_loader::{LoaderTile, TileLoader, VectorTileEncoding};
use crate::tile::tile_observer::TileObserver;
use crate::tile::tile_worker_transport::{TileEndpoint, WorkerEndpoint};
use crate::tile_mask::TileMask;
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::{SharedBucketTileRenderData, TileRenderData};

/// The messages a `RasterTile` sends its `RasterTileWorker`: the worker's
/// public member functions. Port-only: serde writes them for a worker in
/// another wasm instance (`tile/worker_codec`).
#[derive(Serialize, Deserialize)]
pub enum RasterWorkerMessage {
    /// `parse(data, correlationID)`.
    Parse {
        #[serde(with = "shared_bytes::option")]
        data: Option<Data>,
        correlation_id: u64,
    },
}

/// The messages a `RasterTileWorker` sends its `RasterTile`: the tile's
/// member functions the worker calls. Port-only: serde writes them for a
/// worker in another wasm instance (`tile/worker_codec`).
#[derive(Serialize, Deserialize)]
pub enum RasterTileMessage {
    /// `onParsed(std::move(bucket), correlationID)`.
    Parsed { bucket: Option<Arc<RasterBucket>>, correlation_id: u64 },
    /// `onError(std::current_exception(), correlationID)`.
    Error { error: Error, correlation_id: u64 },
}

/// `RasterTileWorker`.
pub struct RasterTileWorker {
    /// The tile, which gets the replies in the order of their parses
    /// although images may decode asynchronously (`ordered_replies.rs`).
    parent: OrderedReplies<RasterTileMessage>,
}

impl RasterTileWorker {
    /// `RasterTileWorker(ActorRef<RasterTileWorker>, ActorRef<RasterTile>)`.
    #[must_use]
    pub fn new(parent: Box<dyn TileEndpoint<RasterTileMessage>>) -> Self {
        RasterTileWorker { parent: OrderedReplies::new(parent) }
    }

    /// Runs the member function a tile's message stands for.
    pub fn handle(&mut self, message: RasterWorkerMessage) {
        match message {
            RasterWorkerMessage::Parse { data, correlation_id } => self.parse(data, correlation_id),
        }
    }

    /// `parse(data, correlationID)`. The reply goes once the image has
    /// decoded, which may be after this returns (decoder.rs in
    /// `mln_image`).
    pub fn parse(&mut self, data: Option<Data>, correlation_id: u64) {
        let parent = self.parent.slot();
        let Some(data) = data else {
            // No data; empty tile.
            parent.send(RasterTileMessage::Parsed { bucket: None, correlation_id });
            return;
        };

        decoder::decode(&data, move |result| match result {
            Ok(image) => {
                let bucket = Arc::new(RasterBucket::new(image));
                parent.send(RasterTileMessage::Parsed { bucket: Some(bucket), correlation_id });
            }
            Err(error) => {
                parent.send(RasterTileMessage::Error { error, correlation_id });
            }
        });
    }
}

/// The render-thread state of a `RasterTile`.
pub struct RasterTileInner {
    pub base: TileBase,
    obsolete: Cell<bool>,
    correlation_id: Cell<u64>,
    /// The tileset has no tile here (`LoaderTile::set_missing`): a parse without data leaves
    /// the tile unrenderable. Otherwise such a tile is empty, renderable without a bucket.
    missing: Cell<bool>,
    bucket: RefCell<Option<Arc<RasterBucket>>>,
    /// `Actor<RasterTileWorker> worker`, behind its transport.
    worker: Box<dyn WorkerEndpoint<RasterWorkerMessage>>,
    mailbox: Arc<Mailbox>,
}

impl RasterTileInner {
    /// Runs the member function a worker's message stands for.
    pub fn handle_worker_message(&self, message: RasterTileMessage) {
        match message {
            RasterTileMessage::Parsed { bucket, correlation_id } => self.on_parsed(bucket, correlation_id),
            RasterTileMessage::Error { error, correlation_id } => self.on_error(error, correlation_id),
        }
    }

    /// `onParsed(std::unique_ptr<RasterBucket>, correlationID)`.
    pub fn on_parsed(&self, result: Option<Arc<RasterBucket>>, result_correlation_id: u64) {
        if !self.obsolete.get() {
            let renderable = result.is_some() || !self.missing.get();
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
    pub fn on_error(&self, err: Error, result_correlation_id: u64) {
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

impl LoaderTile for RasterTileInner {
    /// `setError(std::exception_ptr)`.
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

    /// `setMetadata(modified, expires)`.
    fn set_metadata(&self, modified: Option<Timestamp>, expires: Option<Timestamp>) {
        self.base.modified.set(modified);
        self.base.expires.set(expires);
    }

    /// `setData(data)`.
    fn set_data(&self, data: Option<Data>, _encoding: VectorTileEncoding) {
        self.parse(data, false);
    }

    fn set_missing(&self, _encoding: VectorTileEncoding) {
        self.parse(None, true);
    }
}

impl RasterTileInner {
    /// Sends the worker `data` to parse, for a tile the tileset is `missing` or has.
    fn parse(&self, data: Option<Data>, missing: bool) {
        if !self.obsolete.get() {
            self.missing.set(missing);
            self.correlation_id.set(self.correlation_id.get() + 1);
            if !self.base.pending.get() {
                self.base.on_tile_action(TileOperation::StartParse);
            }
            self.base.pending.set(true);
            let correlation_id = self.correlation_id.get();
            self.worker.send(RasterWorkerMessage::Parse { data, correlation_id });
        }
    }
}

/// `RasterTile`.
pub struct RasterTile {
    inner: Rc<RasterTileInner>,
    loader: Option<TileLoader>,
}

impl RasterTile {
    /// `RasterTile(id, sourceID, parameters, tileset, observer)`.
    #[must_use]
    pub fn new(
        id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        tileset: &Tileset,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let mailbox = Mailbox::new();
        let parent: ActorRef<RasterTileInner> = ActorRef::new(&mailbox);
        let worker = parameters.tile_worker_transport.create_raster(Box::new(parent));
        let inner = Rc::new(RasterTileInner {
            base: TileBase::new(Kind::Raster, id, source_id, observer),
            obsolete: Cell::new(false),
            correlation_id: Cell::new(0),
            missing: Cell::new(false),
            bucket: RefCell::new(None),
            worker,
            mailbox: mailbox.clone(),
        });
        mailbox.open(&scheduler::get_current(), inner.clone() as Rc<dyn Any>);
        let weak: std::rc::Weak<RasterTileInner> = Rc::downgrade(&inner);
        let loader = TileLoader::new(weak, VectorTileEncoding::Mvt, &id, parameters, tileset);
        RasterTile { inner, loader: Some(loader) }
    }

    #[must_use]
    pub fn inner(&self) -> &Rc<RasterTileInner> {
        &self.inner
    }
}

impl Drop for RasterTile {
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

impl Tile for RasterTile {
    fn base(&self) -> &TileBase {
        &self.inner.base
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

    fn retry_failed_load(&self) {
        if let Some(loader) = &self.loader {
            loader.retry_failed_load();
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
