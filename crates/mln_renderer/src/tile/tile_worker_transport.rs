// Port-only: how a raster or raster-dem tile reaches its worker. Geometry
// tiles submit jobs through `TileWorkers` (tile_workers.rs) instead.
//
// A raster tile and its worker talk only through messages, one enum each way
// per worker type (`RasterWorkerMessage` and `RasterTileMessage` in
// raster_tile.rs, `RasterDEMWorkerMessage` and `RasterDEMTileMessage` in
// raster_dem_tile.rs), handed to a `TileWorkerTransport` that the
// RenderOrchestrator owns. The transport creates the worker for a tile and
// returns the `WorkerEndpoint` the tile sends to; the worker's replies go to
// the `TileEndpoint` the tile hands over.
//
// `InProcessTransport` puts the worker on the map's thread pool, as C++
// does. `RemoteWorkers` (worker_codec/transport.rs) puts it in another
// worker instance (the browser's Web Workers) and sends the messages as
// bytes.

use mln_actor::actor::{Actor, PoolActorRef};
use mln_actor::{ActorRef, TaggedScheduler};

use crate::tile::raster_dem_tile::{
    RasterDEMTileInner, RasterDEMTileMessage, RasterDEMTileWorker, RasterDEMWorkerMessage,
};
use crate::tile::raster_tile::{RasterTileInner, RasterTileMessage, RasterTileWorker, RasterWorkerMessage};

/// Where a worker's messages of type `M` for its tile go.
pub(crate) trait TileEndpoint<M>: Send {
    fn send(&self, message: M);
}

/// Where a tile's messages of type `M` for its worker go. Dropping it drops
/// the worker.
pub(crate) trait WorkerEndpoint<M> {
    fn send(&self, message: M);
}

/// Creates the worker of each `RasterTile` and `RasterDEMTile`.
pub(crate) trait TileWorkerTransport {
    /// Creates the `RasterTileWorker` of a `RasterTile`; its replies go to
    /// `tile`.
    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>>;

    /// Creates the `RasterDEMTileWorker` of a `RasterDEMTile`; its replies go
    /// to `tile`.
    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>>;
}

/// Replies go to the tile's mailbox.
impl TileEndpoint<RasterTileMessage> for ActorRef<RasterTileInner> {
    fn send(&self, message: RasterTileMessage) {
        self.invoke(move |tile: &RasterTileInner| tile.handle_worker_message(message));
    }
}

/// Replies go to the tile's mailbox.
impl TileEndpoint<RasterDEMTileMessage> for ActorRef<RasterDEMTileInner> {
    fn send(&self, message: RasterDEMTileMessage) {
        self.invoke(move |tile: &RasterDEMTileInner| tile.handle_worker_message(message));
    }
}

/// Workers on the map's thread pool, as in C++.
pub(crate) struct InProcessTransport {
    thread_pool: TaggedScheduler,
}

impl InProcessTransport {
    #[must_use]
    pub(crate) fn new(thread_pool: TaggedScheduler) -> Self {
        InProcessTransport { thread_pool }
    }
}

impl TileWorkerTransport for InProcessTransport {
    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>> {
        Box::new(PoolWorker::new(Actor::new_tagged(&self.thread_pool, move |_| RasterTileWorker::new(tile))))
    }

    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>> {
        Box::new(PoolWorker::new(Actor::new_tagged(&self.thread_pool, move |_| RasterDEMTileWorker::new(tile))))
    }
}

/// The `Actor` of a raster or DEM tile's worker on the thread pool.
struct PoolWorker<W: Send + 'static> {
    actor_ref: PoolActorRef<W>,
    _actor: Actor<W>,
}

impl<W: Send + 'static> PoolWorker<W> {
    fn new(actor: Actor<W>) -> Self {
        PoolWorker { actor_ref: actor.self_ref(), _actor: actor }
    }
}

impl WorkerEndpoint<RasterWorkerMessage> for PoolWorker<RasterTileWorker> {
    fn send(&self, message: RasterWorkerMessage) {
        self.actor_ref.invoke(move |worker: &mut RasterTileWorker| worker.handle(message));
    }
}

impl WorkerEndpoint<RasterDEMWorkerMessage> for PoolWorker<RasterDEMTileWorker> {
    fn send(&self, message: RasterDEMWorkerMessage) {
        self.actor_ref.invoke(move |worker: &mut RasterDEMTileWorker| worker.handle(message));
    }
}
