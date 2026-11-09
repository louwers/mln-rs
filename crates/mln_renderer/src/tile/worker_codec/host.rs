// Port-only: the tile workers of one worker instance, behind bytes (mod.rs).
//
// A `TileWorkerHost` is what a worker instance runs: it decodes the map
// side's requests with the instance's `WorkerCodec`, keeps a worker (a
// `GeometryTileWorker`, `RasterTileWorker` or `RasterDEMTileWorker`) for
// each tile the map side created there, and encodes what the workers send
// their tiles. It creates the workers as `InProcessTransport` does, so they
// are actors on the scheduler the host is given, and a GeometryTileWorker's
// own messages coalesce as in C++.
//
// A `setData` or `setLayers` the host can't decode becomes an error for the
// tile, as a parse that throws does in C++, where the worker then keeps the
// data or layers it couldn't parse, so that it lays out nothing until the
// tile sends others. Here the worker never got them, so the host resets it
// (which drops both the data and the layers) and gives it back the other of
// the two, as its tile last sent it. Until the tile sends what failed again,
// each `setData` or `setLayers` gets that error, as each parse would throw in
// C++, rather than no answer, which would leave the tile pending.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mln_actor::TaggedScheduler;
use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_util::exception::RuntimeError;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};

use super::envelope::{About, Encoded, MessageKind};
use super::worker_side::{WorkerAction, WorkerCodec};
use crate::tile::geometry_tile_worker::{SharedTileData, TileMessage, WorkerMessage};
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use crate::tile::tile_worker_transport::{InProcessTransport, TileEndpoint, TileWorkerTransport, WorkerEndpoint};

/// Where a host's replies go, encoded: back to the map side of its
/// instance. Their payloads (pixels, vertices, indices) can be transferred
/// there rather than copied.
pub type Replies = Arc<dyn Fn(Encoded) + Send + Sync>;

/// The tile workers of one worker instance.
pub struct TileWorkerHost {
    codec: Arc<Mutex<WorkerCodec>>,
    /// Creates the workers.
    workers: InProcessTransport,
    replies: Replies,
    hosted: RefCell<HashMap<u64, Hosted>>,
}

/// A message for a worker, decoded as its type's.
enum Decoded {
    Geometry(WorkerMessage),
    Raster(RasterWorkerMessage),
    RasterDEM(RasterDEMWorkerMessage),
}

/// A tile's worker.
enum Hosted {
    Geometry {
        /// The worker's `obsolete` flag, set when the map side drops the
        /// worker.
        obsolete: Arc<AtomicBool>,
        worker: Box<dyn WorkerEndpoint>,
        /// What the worker has of its tile's data and layers.
        sent: Sent,
    },
    Raster(Box<dyn WorkerEndpoint<RasterWorkerMessage>>),
    RasterDEM(Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>>),
}

/// What a geometry tile's worker has of its tile's data and of its layers.
#[derive(Default)]
struct Sent {
    data: Kept<(Option<SharedTileData>, Immutable<BTreeSet<String>>)>,
    layers: Kept<(Vec<Immutable<dyn LayerProperties>>, Option<Arc<GlobalStateMap>>, Immutable<BTreeSet<String>>)>,
}

/// The tile's data or its layers, as the worker last got them, with the
/// available images of that message.
#[derive(Default)]
enum Kept<T> {
    #[default]
    Nothing,
    Sent(T),
    /// The message couldn't be decoded, with this error.
    Failed(String),
}

impl<T> Kept<T> {
    fn error(&self) -> Option<String> {
        match self {
            Kept::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }
}

impl Sent {
    /// Keeps what `message` gives the worker. Returns the error its tile
    /// gets for it: that of the data or layers the worker lacks because
    /// they failed, with the message's correlation id.
    fn keep(&mut self, message: &WorkerMessage) -> Option<(String, u64)> {
        match message {
            WorkerMessage::SetData { data, available_images, correlation_id } => {
                self.data = Kept::Sent((data.clone(), available_images.clone()));
                self.layers.error().map(|error| (error, *correlation_id))
            }
            WorkerMessage::SetLayers { layers, global_state, available_images, correlation_id } => {
                self.layers = Kept::Sent((layers.clone(), global_state.clone(), available_images.clone()));
                self.data.error().map(|error| (error, *correlation_id))
            }
            WorkerMessage::Reset { .. } => {
                *self = Sent::default();
                None
            }
            _ => None,
        }
    }

    /// Records that the `setData` or `setLayers` with `correlation_id`
    /// failed with `error`. Returns the message that gives the worker, once
    /// reset, back the other of the two.
    fn fail(&mut self, failed: MessageKind, correlation_id: u64, error: String) -> Option<WorkerMessage> {
        match failed {
            MessageKind::SetData => {
                self.data = Kept::Failed(error);
                match &self.layers {
                    Kept::Sent((layers, global_state, available_images)) => Some(WorkerMessage::SetLayers {
                        layers: layers.clone(),
                        global_state: global_state.clone(),
                        available_images: available_images.clone(),
                        correlation_id,
                    }),
                    _ => None,
                }
            }
            MessageKind::SetLayers => {
                self.layers = Kept::Failed(error);
                match &self.data {
                    Kept::Sent((data, available_images)) => Some(WorkerMessage::SetData {
                        data: data.clone(),
                        available_images: available_images.clone(),
                        correlation_id,
                    }),
                    _ => None,
                }
            }
            MessageKind::Other => None,
        }
    }
}

impl TileWorkerHost {
    /// A host whose workers are actors on `workers`; it sends its replies to
    /// `replies`.
    pub fn new(workers: TaggedScheduler, replies: impl Fn(Encoded) + Send + Sync + 'static) -> Self {
        TileWorkerHost {
            codec: Arc::new(Mutex::new(WorkerCodec::new())),
            workers: InProcessTransport::new(workers),
            replies: Arc::new(replies),
            hosted: RefCell::new(HashMap::new()),
        }
    }

    /// Handles a request from the map side.
    pub fn handle(&self, request: &[u8]) {
        let action = self.codec.lock().unwrap_or_else(|e| e.into_inner()).decode(request);
        match action {
            Ok(WorkerAction::Create { tile, parameters }) => {
                let obsolete = parameters.obsolete.clone();
                let worker = self.workers.create(parameters, Box::new(self.tile(tile)));
                self.hosted.borrow_mut().insert(tile, Hosted::Geometry { obsolete, worker, sent: Sent::default() });
            }
            Ok(WorkerAction::CreateRaster { tile }) => {
                let worker = self.workers.create_raster(Box::new(self.tile(tile)));
                self.hosted.borrow_mut().insert(tile, Hosted::Raster(worker));
            }
            Ok(WorkerAction::CreateRasterDEM { tile }) => {
                let worker = self.workers.create_raster_dem(Box::new(self.tile(tile)));
                self.hosted.borrow_mut().insert(tile, Hosted::RasterDEM(worker));
            }
            Ok(WorkerAction::Message { tile, about, message }) => self.message(tile, about, &message),
            Ok(WorkerAction::Remove { tile }) => {
                let hosted = self.hosted.borrow_mut().remove(&tile);
                if let Some(Hosted::Geometry { obsolete, .. }) = hosted {
                    obsolete.store(true, Ordering::SeqCst);
                }
            }
            Ok(WorkerAction::None) => {}
            Err(error) => Log::error(Event::General, format!("Not a tile worker request: {error}")),
        }
    }

    /// Decodes a message for the worker of `tile` and sends it there. One
    /// that can't be decoded becomes an error for the tile.
    fn message(&self, tile: u64, about: About, message: &[u8]) {
        let correlation_id = about.correlation_id;
        let decoded = {
            let mut codec = self.codec.lock().unwrap_or_else(|e| e.into_inner());
            match self.hosted.borrow().get(&tile) {
                Some(Hosted::Geometry { .. }) => codec.decode_message(message).map(Decoded::Geometry),
                Some(Hosted::Raster(_)) => codec.decode_message(message).map(Decoded::Raster),
                Some(Hosted::RasterDEM(_)) => codec.decode_message(message).map(Decoded::RasterDEM),
                None => return,
            }
        };
        let error = match decoded {
            Ok(Decoded::Geometry(message)) => match self.hosted.borrow_mut().get_mut(&tile) {
                Some(Hosted::Geometry { worker, sent, .. }) => {
                    let error = sent.keep(&message);
                    worker.send(message);
                    error
                }
                _ => None,
            },
            Ok(Decoded::Raster(message)) => {
                if let Some(Hosted::Raster(worker)) = self.hosted.borrow().get(&tile) {
                    worker.send(message);
                }
                None
            }
            Ok(Decoded::RasterDEM(message)) => {
                if let Some(Hosted::RasterDEM(worker)) = self.hosted.borrow().get(&tile) {
                    worker.send(message);
                }
                None
            }
            Err(error) => {
                if let Some(Hosted::Geometry { worker, sent, .. }) = self.hosted.borrow_mut().get_mut(&tile)
                    && about.kind != MessageKind::Other
                {
                    worker.send(WorkerMessage::Reset { correlation_id });
                    if let Some(restore) = sent.fail(about.kind, correlation_id, error.clone()) {
                        worker.send(restore);
                    }
                }
                Some((error, correlation_id))
            }
        };
        let Some((error, correlation_id)) = error else {
            return;
        };
        let error = RuntimeError::ptr(error);
        let endpoint = self.tile(tile);
        match self.hosted.borrow().get(&tile) {
            Some(Hosted::Geometry { .. }) => endpoint.send(TileMessage::Error { error, correlation_id }),
            Some(Hosted::Raster(_)) => endpoint.send(RasterTileMessage::Error { error, correlation_id }),
            Some(Hosted::RasterDEM(_)) => endpoint.send(RasterDEMTileMessage::Error { error, correlation_id }),
            None => {}
        }
    }

    /// The endpoint through which the worker of `tile` reaches its tile.
    fn tile(&self, tile: u64) -> EncodingTile {
        EncodingTile { tile, codec: self.codec.clone(), replies: self.replies.clone() }
    }
}

/// A worker's tile, on the map side: its messages are encoded and sent
/// there.
struct EncodingTile {
    tile: u64,
    codec: Arc<Mutex<WorkerCodec>>,
    replies: Replies,
}

impl TileEndpoint for EncodingTile {
    fn send(&self, message: TileMessage) {
        let reply = self.codec.lock().unwrap_or_else(|e| e.into_inner()).encode(self.tile, message);
        (self.replies)(reply);
    }
}

impl TileEndpoint<RasterTileMessage> for EncodingTile {
    fn send(&self, message: RasterTileMessage) {
        let reply = self.codec.lock().unwrap_or_else(|e| e.into_inner()).encode_raster(self.tile, message);
        (self.replies)(reply);
    }
}

impl TileEndpoint<RasterDEMTileMessage> for EncodingTile {
    fn send(&self, message: RasterDEMTileMessage) {
        let reply = self.codec.lock().unwrap_or_else(|e| e.into_inner()).encode_raster_dem(self.tile, message);
        (self.replies)(reply);
    }
}
