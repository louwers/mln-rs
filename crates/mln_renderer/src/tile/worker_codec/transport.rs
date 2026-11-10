// Port-only: a `TileWorkerTransport` whose workers are in other worker
// instances, reached only through bytes (mod.rs).
//
// `SerializingTransport` is the map side. Each tile's worker (geometry,
// raster or raster-dem) goes to one of the `WorkerInstances`, in turn, and
// every message between the two crosses as bytes through that instance's
// `MapCodec`; each instance runs a `TileWorkerHost` (host.rs). The geometry
// tiles of sources with synchronous updates, which must be laid out before
// the call that updates them returns, keep their worker on the map's
// thread, called directly as `InProcessTransport` does.
//
// In the browser each instance is a Web Worker with its own wasm instance
// and memory. `ThreadWorkerInstances` runs the instances on threads of this
// process instead, each host with its workers on its thread's RunLoop and
// nothing shared with the map but the bytes; the render-test runner uses it
// (`--tileWorkerCodec`) to run the suites through the codec.
//
// The tile's `obsolete` flag can't cross: a host sets its worker's flag when
// the tile drops the worker, and a tile that is obsolete before then no
// longer takes replies (its mailbox is closed).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_actor::{LocalObject, SchedulerPtr, TaggedScheduler, Thread, scheduler};
use mln_util::exception::RuntimeError;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};

use super::TileWorkerHost;
use super::envelope::Encoded;
use super::map_side::{MapCodec, Reply};
use crate::tile::geometry_tile_worker::{TileMessage, WorkerMessage};
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use crate::tile::tile_worker_transport::{
    InProcessTransport, TileEndpoint, TileWorkerTransport, WorkerEndpoint, WorkerParameters,
};

/// The worker instances a `SerializingTransport` sends its requests to.
/// Their replies come back through the `ReplySink` the transport gave
/// whoever created them, or `SerializingTransport::receive`.
pub trait WorkerInstances {
    /// How many there are; requests go to `0..count()`.
    fn count(&self) -> usize;
    /// Sends a request to the host of instance `instance`.
    fn post(&self, instance: usize, request: Vec<u8>);
}

/// Delivers worker instances' replies to their `SerializingTransport`, from
/// any thread: each reply is handled by a task on the transport's thread,
/// in the order they were sent.
#[derive(Clone)]
pub struct ReplySink {
    scheduler: SchedulerPtr,
    transport: Arc<LocalObject<Weak<SerializingTransport>>>,
}

impl ReplySink {
    /// Delivers a reply of instance `instance`.
    pub fn send(&self, instance: usize, reply: Encoded) {
        let transport = self.transport.clone();
        self.scheduler.schedule(Box::new(move || {
            if let Some(transport) = transport.get().and_then(|weak| weak.upgrade()) {
                transport.receive(instance, reply);
            }
        }));
    }
}

/// The map side of tile workers in other worker instances.
pub struct SerializingTransport {
    self_weak: Weak<SerializingTransport>,
    instances: Box<dyn WorkerInstances>,
    /// The map side of each instance.
    codecs: Vec<RefCell<MapCodec>>,
    /// Creates the workers of synchronous updates, on this thread.
    synchronous: InProcessTransport,
    tiles: RefCell<HashMap<u64, Route>>,
    next_tile: Cell<u64>,
    next_instance: Cell<usize>,
}

/// Where a tile's worker is, and the tile's endpoint for its replies.
struct Route {
    instance: usize,
    tile: RouteTile,
}

/// A tile's endpoint, of its worker type. Shared so that a reply is
/// delivered without the routes borrowed.
#[derive(Clone)]
enum RouteTile {
    Geometry(Rc<dyn TileEndpoint>),
    Raster(Rc<dyn TileEndpoint<RasterTileMessage>>),
    RasterDEM(Rc<dyn TileEndpoint<RasterDEMTileMessage>>),
}

impl SerializingTransport {
    /// A transport over the instances `instances` creates, with the sink
    /// for their replies. The replies are handled on the calling thread
    /// (its scheduler runs them), which must be the one that uses the
    /// transport.
    pub fn new(instances: impl FnOnce(ReplySink) -> Box<dyn WorkerInstances>) -> Rc<Self> {
        Rc::new_cyclic(|self_weak: &Weak<SerializingTransport>| {
            let transport = Arc::new(LocalObject::new(self_weak.clone()));
            let instances = instances(ReplySink { scheduler: scheduler::get_current(), transport });
            let count = instances.count();
            assert!(count > 0, "a SerializingTransport needs a worker instance");
            // Its workers are called directly, so it schedules nothing.
            let synchronous = TaggedScheduler::new(scheduler::get_current(), SimpleIdentity::new());
            SerializingTransport {
                self_weak: self_weak.clone(),
                instances,
                codecs: (0..count).map(|_| RefCell::new(MapCodec::new())).collect(),
                synchronous: InProcessTransport::new(synchronous),
                tiles: RefCell::new(HashMap::new()),
                next_tile: Cell::new(0),
                next_instance: Cell::new(0),
            }
        })
    }

    /// Handles a reply from instance `instance` (for a host that gets the
    /// replies on this thread itself rather than through the `ReplySink`):
    /// its bytes and its payloads, which the buckets and images it carries
    /// take as they are.
    pub fn receive(&self, instance: usize, reply: Encoded) {
        let decoded = self.codecs[instance].borrow_mut().decode(reply);
        match decoded {
            Ok(Some((tile, message))) => self.deliver(tile, message),
            // The tile dropped its worker after the worker sent this.
            Ok(None) => {}
            Err(error) => Log::error(Event::General, format!("Not a tile worker reply: {error}")),
        }
        // What the layouts the tiles replaced released, so the instance can
        // reuse those bins without waiting for the next request.
        let releases = self.codecs[instance].borrow_mut().releases();
        if let Some(releases) = releases {
            self.instances.post(instance, releases);
        }
    }

    fn deliver(&self, tile: u64, message: Reply) {
        let endpoint = self.tiles.borrow().get(&tile).map(|route| route.tile.clone());
        match (endpoint, message) {
            (Some(RouteTile::Geometry(endpoint)), Reply::Geometry(message)) => endpoint.send(message),
            (Some(RouteTile::Raster(endpoint)), Reply::Raster(message)) => endpoint.send(message),
            (Some(RouteTile::RasterDEM(endpoint)), Reply::RasterDEM(message)) => endpoint.send(message),
            // The codec decodes a tile's replies as its worker type's.
            (Some(_), _) => Log::error(Event::General, format!("A reply of another worker type for tile {tile}")),
            (None, _) => {}
        }
    }

    fn instance(&self, tile: u64) -> Option<usize> {
        self.tiles.borrow().get(&tile).map(|route| route.instance)
    }

    fn send(&self, tile: u64, message: WorkerMessage) {
        let Some(instance) = self.instance(tile) else {
            return;
        };
        let correlation_id = message.correlation_id();
        let request = self.codecs[instance].borrow_mut().message(tile, message);
        match request {
            Ok(request) => self.instances.post(instance, request),
            // As a worker reports what it can't parse.
            Err(error) => self
                .deliver(tile, Reply::Geometry(TileMessage::Error { error: RuntimeError::ptr(error), correlation_id })),
        }
    }

    fn send_raster(&self, tile: u64, message: RasterWorkerMessage) {
        if let Some(instance) = self.instance(tile) {
            let request = self.codecs[instance].borrow_mut().raster_message(tile, message);
            self.instances.post(instance, request);
        }
    }

    fn send_raster_dem(&self, tile: u64, message: RasterDEMWorkerMessage) {
        if let Some(instance) = self.instance(tile) {
            let request = self.codecs[instance].borrow_mut().raster_dem_message(tile, message);
            self.instances.post(instance, request);
        }
    }

    /// Gives a new tile's worker to the next instance, with the request
    /// `create` makes for it there; returns the tile's worker.
    fn add(&self, tile: RouteTile, create: impl FnOnce(&mut MapCodec, u64) -> Vec<u8>) -> RemoteWorker {
        let key = self.next_tile.get();
        self.next_tile.set(key + 1);
        let instance = self.next_instance.get();
        self.next_instance.set((instance + 1) % self.instances.count());
        let request = create(&mut self.codecs[instance].borrow_mut(), key);
        self.tiles.borrow_mut().insert(key, Route { instance, tile });
        self.instances.post(instance, request);
        let transport = self.self_weak.upgrade().expect("the transport creates workers while it lives");
        RemoteWorker { transport, tile: key }
    }

    fn remove(&self, tile: u64) {
        let route = self.tiles.borrow_mut().remove(&tile);
        if let Some(route) = route {
            let request = self.codecs[route.instance].borrow_mut().remove(tile);
            self.instances.post(route.instance, request);
        }
    }
}

impl TileWorkerTransport for SerializingTransport {
    fn create(&self, parameters: WorkerParameters, tile: Box<dyn TileEndpoint>) -> Box<dyn WorkerEndpoint> {
        if parameters.synchronous {
            return self.synchronous.create(parameters, tile);
        }
        Box::new(self.add(RouteTile::Geometry(Rc::from(tile)), |codec, key| codec.create(key, &parameters)))
    }

    fn create_raster(
        &self,
        tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>> {
        Box::new(self.add(RouteTile::Raster(Rc::from(tile)), MapCodec::create_raster))
    }

    fn create_raster_dem(
        &self,
        tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>> {
        Box::new(self.add(RouteTile::RasterDEM(Rc::from(tile)), MapCodec::create_raster_dem))
    }
}

/// A tile's worker in a worker instance. Dropping it drops the worker.
struct RemoteWorker {
    transport: Rc<SerializingTransport>,
    tile: u64,
}

impl WorkerEndpoint for RemoteWorker {
    fn send(&self, message: WorkerMessage) {
        self.transport.send(self.tile, message);
    }
}

impl WorkerEndpoint<RasterWorkerMessage> for RemoteWorker {
    fn send(&self, message: RasterWorkerMessage) {
        self.transport.send_raster(self.tile, message);
    }
}

impl WorkerEndpoint<RasterDEMWorkerMessage> for RemoteWorker {
    fn send(&self, message: RasterDEMWorkerMessage) {
        self.transport.send_raster_dem(self.tile, message);
    }
}

impl Drop for RemoteWorker {
    fn drop(&mut self) {
        self.transport.remove(self.tile);
    }
}

/// Worker instances on threads of this process: a `TileWorkerHost` on each
/// thread, with its workers on the thread's RunLoop.
pub struct ThreadWorkerInstances {
    threads: Vec<Thread<TileWorkerHost>>,
}

impl ThreadWorkerInstances {
    /// `count` instances that send their replies to `replies`.
    pub fn new(count: usize, replies: ReplySink) -> Self {
        let threads = (0..count)
            .map(|instance| {
                let replies = replies.clone();
                Thread::new(&format!("TileWorker {instance}"), move |_| {
                    let workers = TaggedScheduler::new(scheduler::get_current(), SimpleIdentity::new());
                    TileWorkerHost::new(workers, move |reply| replies.send(instance, reply))
                })
            })
            .collect();
        ThreadWorkerInstances { threads }
    }
}

impl WorkerInstances for ThreadWorkerInstances {
    fn count(&self) -> usize {
        self.threads.len()
    }

    fn post(&self, instance: usize, request: Vec<u8>) {
        self.threads[instance].actor().invoke(move |host: &TileWorkerHost| host.handle(&request));
    }
}
