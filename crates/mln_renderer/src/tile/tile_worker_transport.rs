// Port-only: how a tile reaches its worker.
//
// C++ `GeometryTile` owns an `OptionalActor<GeometryTileWorker>`,
// `RasterTile` an `Actor<RasterTileWorker>` and `RasterDEMTile` an
// `Actor<RasterDEMTileWorker>`, and each tile and its worker talk through
// member function calls bound into actor messages. Here they talk only
// through messages, one enum each way per worker type (`WorkerMessage` and
// `TileMessage` in geometry_tile_worker.rs, `RasterWorkerMessage` and
// `RasterTileMessage` in raster_tile.rs, `RasterDEMWorkerMessage` and
// `RasterDEMTileMessage` in raster_dem_tile.rs), handed to a
// `TileWorkerTransport` that the RenderOrchestrator owns. The transport
// creates the worker for a tile and returns the `WorkerEndpoint` the tile
// sends to; the worker's replies go to the `TileEndpoint` the tile hands
// over.
//
// `InProcessTransport` is C++'s arrangement: the worker is an actor on the
// map's tagged thread pool, or (synchronous updates of geometry tiles) an
// object the tile calls directly, and messages pass by value.
// `SerializingTransport` (worker_codec/transport.rs) puts the workers in
// other worker instances (the browser's Web Workers) and sends the messages
// as bytes.

use std::cell::RefCell;
use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::{ActorRef, LocalObject, TaggedScheduler};
use mln_map_types::MapMode;
use mln_text::glyph::FontFaces;
use mln_tile_id::OverscaledTileID;
use mln_util::logging::{Event, Log};
use serde::{Deserialize, Serialize};

use crate::gfx::dynamic_texture_atlas::DynamicTextureAtlasPtr;
use crate::tile::geometry_tile::GeometryTileInner;
use crate::tile::geometry_tile_worker::{GeometryTileWorker, TileMessage, WorkerMessage};
use crate::tile::raster_dem_tile::{
    RasterDEMTileInner, RasterDEMTileMessage, RasterDEMTileWorker, RasterDEMWorkerMessage,
};
use crate::tile::raster_tile::{RasterTileInner, RasterTileMessage, RasterTileWorker, RasterWorkerMessage};

/// The arguments of the C++ `GeometryTileWorker` constructor that a tile
/// passes, plus whether the worker runs synchronously
/// (`TileParameters::isUpdateSynchronous`).
///
/// Serde writes them for a worker in another worker instance (worker_codec),
/// without what can't cross: there, the instance's host gives the worker an
/// `obsolete` flag and an atlas of its own, and the worker runs as an actor.
#[derive(Clone, Serialize, Deserialize)]
pub struct WorkerParameters {
    pub id: OverscaledTileID,
    pub source_id: String,
    /// Set by the tile when it no longer needs the worker's results.
    #[serde(skip)]
    pub obsolete: Arc<AtomicBool>,
    pub mode: MapMode,
    pub pixel_ratio: f32,
    pub show_collision_boxes: bool,
    #[serde(skip)]
    pub dynamic_texture_atlas: Option<DynamicTextureAtlasPtr>,
    pub font_faces: Option<Arc<FontFaces>>,
    pub capture_rendered_features: bool,
    #[serde(skip)]
    pub synchronous: bool,
}

/// Where a worker's messages for its tile go: `TileMessage`s from a
/// GeometryTileWorker, or `M` from another worker type.
pub trait TileEndpoint<M = TileMessage>: Send {
    fn send(&self, message: M);
}

/// Where a tile's messages for its worker go: `WorkerMessage`s for a
/// GeometryTileWorker, or `M` for another worker type. Dropping it drops the
/// worker.
pub trait WorkerEndpoint<M = WorkerMessage> {
    fn send(&self, message: M);
}

/// Creates the worker of each GeometryTile, RasterTile and RasterDEMTile.
pub trait TileWorkerTransport {
    /// Creates the worker for a GeometryTile; its replies go to `tile`. With
    /// `parameters.synchronous`, `tile` is called directly on the calling
    /// thread.
    fn create(&self, parameters: WorkerParameters, tile: Box<dyn TileEndpoint>) -> Box<dyn WorkerEndpoint>;

    /// Creates the `RasterTileWorker` of a RasterTile; its replies go to
    /// `tile`.
    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>>;

    /// Creates the `RasterDEMTileWorker` of a RasterDEMTile; its replies go
    /// to `tile`.
    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>>;
}

/// `ActorRef<GeometryTile>`: the tile's mailbox.
impl TileEndpoint for ActorRef<GeometryTileInner> {
    fn send(&self, message: TileMessage) {
        self.invoke(move |tile: &GeometryTileInner| tile.handle_worker_message(message));
    }
}

/// `ActorRef<RasterTile>`: the tile's mailbox.
impl TileEndpoint<RasterTileMessage> for ActorRef<RasterTileInner> {
    fn send(&self, message: RasterTileMessage) {
        self.invoke(move |tile: &RasterTileInner| tile.handle_worker_message(message));
    }
}

/// `ActorRef<RasterDEMTile>`: the tile's mailbox.
impl TileEndpoint<RasterDEMTileMessage> for ActorRef<RasterDEMTileInner> {
    fn send(&self, message: RasterDEMTileMessage) {
        self.invoke(move |tile: &RasterDEMTileInner| tile.handle_worker_message(message));
    }
}

/// The synchronous `OptionalActorRef<GeometryTile>`: the tile itself, on
/// the render thread.
pub struct SynchronousTile(pub LocalObject<Weak<GeometryTileInner>>);

impl TileEndpoint for SynchronousTile {
    fn send(&self, message: TileMessage) {
        if let Some(tile) = self.0.get().and_then(|weak| weak.upgrade()) {
            tile.handle_worker_message(message);
        }
    }
}

/// Workers on the map's thread pool, as in C++.
pub struct InProcessTransport {
    thread_pool: TaggedScheduler,
}

impl InProcessTransport {
    pub fn new(thread_pool: TaggedScheduler) -> Self {
        InProcessTransport { thread_pool }
    }
}

impl TileWorkerTransport for InProcessTransport {
    fn create(&self, parameters: WorkerParameters, tile: Box<dyn TileEndpoint>) -> Box<dyn WorkerEndpoint> {
        let WorkerParameters {
            id,
            source_id,
            obsolete,
            mode,
            pixel_ratio,
            show_collision_boxes,
            dynamic_texture_atlas,
            font_faces,
            capture_rendered_features,
            synchronous,
        } = parameters;
        let make_worker = move |self_ref| {
            GeometryTileWorker::new(
                self_ref,
                tile,
                id,
                source_id,
                obsolete,
                mode,
                pixel_ratio,
                show_collision_boxes,
                dynamic_texture_atlas,
                font_faces,
                capture_rendered_features,
            )
        };
        if synchronous {
            Box::new(SynchronousWorker(RefCell::new(make_worker(None))))
        } else {
            Box::new(PoolWorker::new(Actor::new_tagged(&self.thread_pool, move |self_ref| make_worker(Some(self_ref)))))
        }
    }

    /// `worker(parameters.threadPool, ActorRef<RasterTile>(*this, mailbox))`.
    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>> {
        Box::new(PoolWorker::new(Actor::new_tagged(&self.thread_pool, move |_| RasterTileWorker::new(tile))))
    }

    /// `worker(parameters.threadPool, ActorRef<RasterDEMTile>(*this, mailbox))`.
    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>> {
        Box::new(PoolWorker::new(Actor::new_tagged(&self.thread_pool, move |_| RasterDEMTileWorker::new(tile))))
    }
}

/// A worker on the thread pool: `OptionalActor<GeometryTileWorker>` with an
/// actor, or the `Actor` of a raster or DEM tile.
struct PoolWorker<W: Send + 'static> {
    actor_ref: PoolActorRef<W>,
    _actor: Actor<W>,
}

impl<W: Send + 'static> PoolWorker<W> {
    fn new(actor: Actor<W>) -> Self {
        PoolWorker { actor_ref: actor.self_ref(), _actor: actor }
    }
}

impl WorkerEndpoint for PoolWorker<GeometryTileWorker> {
    /// `worker.self().invoke(fn, args...)`.
    fn send(&self, message: WorkerMessage) {
        self.actor_ref.invoke(move |worker: &mut GeometryTileWorker| worker.handle(message));
    }
}

impl WorkerEndpoint<RasterWorkerMessage> for PoolWorker<RasterTileWorker> {
    /// `worker.self().invoke(fn, args...)`.
    fn send(&self, message: RasterWorkerMessage) {
        self.actor_ref.invoke(move |worker: &mut RasterTileWorker| worker.handle(message));
    }
}

impl WorkerEndpoint<RasterDEMWorkerMessage> for PoolWorker<RasterDEMTileWorker> {
    /// `worker.self().invoke(fn, args...)`.
    fn send(&self, message: RasterDEMWorkerMessage) {
        self.actor_ref.invoke(move |worker: &mut RasterDEMTileWorker| worker.handle(message));
    }
}

/// `OptionalActor<GeometryTileWorker>` with an object: the worker on the
/// render thread, for synchronous updates.
struct SynchronousWorker(RefCell<GeometryTileWorker>);

impl WorkerEndpoint for SynchronousWorker {
    /// `worker.self().invoke(fn, args...)`: a direct call.
    fn send(&self, message: WorkerMessage) {
        match self.0.try_borrow_mut() {
            Ok(mut worker) => worker.handle(message),
            // C++ makes the nested call (e.g. onImagesAvailable from inside
            // parse); the port cannot re-enter the worker.
            // TODO(port): reentrant synchronous calls.
            Err(_) => Log::error(Event::General, "TODO(port): reentrant call into a synchronous GeometryTileWorker"),
        }
    }
}
