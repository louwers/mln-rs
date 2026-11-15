// Port-only: what one worker instance runs, behind bytes (mod.rs).
//
// A `TileWorkerHost` decodes the map side's frames. It runs each job as its
// frame arrives, one job per frame, and sends the reply back before it
// returns; it keeps the tile data of the jobs in a cache of its own, which a
// later job can name instead of carrying the data. A job whose body it
// can't decode gets a `Failed` reply with the header's id, and a frame
// whose header it can't read an empty reply, which the map side takes as
// the instance lost.
//
// Raster and raster-dem tiles keep a worker each here, made as
// `InProcessTransport` makes them, so they are actors on the scheduler the
// host is given.

use std::cell::RefCell;
use std::sync::Arc;

use mln_actor::TaggedScheduler;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, Log};

use super::envelope::{self, Action, Encoded, JOB, RASTER};
use super::worker_side::{WorkerCodec, encode_raster_reply};
use crate::tile::job::{DEFAULT_DATA_CACHE_BYTES, DataCache, DiscoveryMemo, JobResult, JobTimes, Reply, Run, run_job};
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use crate::tile::tile_worker_transport::{InProcessTransport, TileEndpoint, TileWorkerTransport, WorkerEndpoint};

/// Where a host's replies go, encoded: back to the map side of its
/// instance. Their payloads (pixels, vertices, indices) can be transferred
/// there rather than copied.
pub(crate) type Replies = Arc<dyn Fn(Encoded) + Send + Sync>;

/// What one worker instance runs.
pub struct TileWorkerHost {
    jobs: RefCell<Jobs>,
    /// Creates the workers of raster and raster-dem tiles.
    workers: InProcessTransport,
    replies: Replies,
    raster: RefCell<HashMap<u64, Hosted>>,
}

/// The jobs' side of an instance.
struct Jobs {
    codec: WorkerCodec,
    cache: DataCache,
    memo: DiscoveryMemo,
}

/// A raster or raster-dem tile's worker.
enum Hosted {
    Raster(Box<dyn WorkerEndpoint<RasterWorkerMessage>>),
    RasterDEM(Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>>),
}

impl TileWorkerHost {
    /// A host whose raster workers are actors on `workers`; it sends its
    /// replies to `replies`. Its data cache keeps 32 MiB.
    pub fn new(workers: TaggedScheduler, replies: impl Fn(Encoded) + Send + Sync + 'static) -> Self {
        TileWorkerHost {
            jobs: RefCell::new(Jobs {
                codec: WorkerCodec::new(),
                cache: DataCache::new(DEFAULT_DATA_CACHE_BYTES),
                // The labelled tiles of a view that resume here, about.
                memo: DiscoveryMemo::new(16),
            }),
            workers: InProcessTransport::new(workers),
            replies: Arc::new(replies),
            raster: RefCell::new(HashMap::default()),
        }
    }

    /// Sets how many bytes of tile data the host keeps for later jobs.
    pub fn set_data_cache_budget(&self, bytes: usize) {
        self.jobs.borrow_mut().cache.set_budget(bytes);
    }

    /// The bytes of tile data the host keeps, by its estimate.
    #[must_use]
    pub fn data_cache_bytes(&self) -> usize {
        self.jobs.borrow().cache.retained_bytes()
    }

    /// Handles a frame from the map side.
    pub fn handle(&self, request: &[u8]) {
        match request.split_first() {
            Some((&JOB, frame)) => self.job(frame),
            Some((&RASTER, frame)) => self.raster(frame),
            _ => self.lost("a tile worker request of no kind"),
        }
    }

    /// Runs the job in `frame` and sends its reply.
    fn job(&self, frame: &[u8]) {
        let Some((id, body)) = envelope::read_job_id(frame) else {
            return self.lost("a tile worker request without a job id");
        };
        let mut jobs = self.jobs.borrow_mut();
        let Jobs { codec, cache, memo } = &mut *jobs;
        let decoded = codec.decode_job(body);
        let replies = &self.replies;
        let mut send = |reply: Reply, times| (replies)(codec.encode_reply(&reply, &times));
        match decoded {
            Ok(job) => run_job(Run { id, job, superseded: Arc::default() }, Some(cache), Some(memo), send),
            Err(error) => {
                let result = JobResult::Failed(Error::native(format!("a tile worker request: {error}")));
                send(Reply { id, result }, JobTimes::default());
            }
        }
    }

    /// Tells the map side that this instance can't go on: a frame it can't
    /// read leaves it out of step with the map side's tables.
    fn lost(&self, error: &str) {
        Log::error(Event::General, format!("Not a tile worker request: {error}"));
        (self.replies)(Encoded::default());
    }

    /// Handles a raster or raster-dem tile's frame.
    fn raster(&self, frame: &[u8]) {
        let (action, message) = match envelope::take::<Action>(frame) {
            Ok(decoded) => decoded,
            Err(error) => return Log::error(Event::General, format!("Not a tile worker request: {error}")),
        };
        match action {
            Action::CreateRaster { tile } => {
                let worker = self.workers.create_raster(Box::new(self.tile(tile)));
                self.raster.borrow_mut().insert(tile, Hosted::Raster(worker));
            }
            Action::CreateRasterDEM { tile } => {
                let worker = self.workers.create_raster_dem(Box::new(self.tile(tile)));
                self.raster.borrow_mut().insert(tile, Hosted::RasterDEM(worker));
            }
            Action::Message { tile, correlation_id } => {
                let raster = self.raster.borrow();
                let endpoint = self.tile(tile);
                match raster.get(&tile) {
                    Some(Hosted::Raster(worker)) => match envelope::decode(message) {
                        Ok(message) => worker.send(message),
                        Err(error) => {
                            endpoint.send(RasterTileMessage::Error { error: Error::native(error), correlation_id });
                        }
                    },
                    Some(Hosted::RasterDEM(worker)) => match envelope::decode(message) {
                        Ok(message) => worker.send(message),
                        Err(error) => {
                            endpoint.send(RasterDEMTileMessage::Error { error: Error::native(error), correlation_id });
                        }
                    },
                    None => {}
                }
            }
            Action::Remove { tile } => drop(self.raster.borrow_mut().remove(&tile)),
        }
    }

    /// The endpoint through which the worker of raster tile `tile` reaches
    /// its tile.
    fn tile(&self, tile: u64) -> EncodingTile {
        EncodingTile { tile, replies: Arc::clone(&self.replies) }
    }
}

/// A raster or raster-dem worker's tile, on the map side: its messages are
/// encoded and sent there.
struct EncodingTile {
    tile: u64,
    replies: Replies,
}

impl TileEndpoint<RasterTileMessage> for EncodingTile {
    fn send(&self, message: RasterTileMessage) {
        let (&RasterTileMessage::Parsed { correlation_id, .. } | &RasterTileMessage::Error { correlation_id, .. }) =
            &message;
        let reply = encode_raster_reply(self.tile, correlation_id, &message, |error| RasterTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        });
        (self.replies)(reply);
    }
}

impl TileEndpoint<RasterDEMTileMessage> for EncodingTile {
    fn send(&self, message: RasterDEMTileMessage) {
        let (&RasterDEMTileMessage::Parsed { correlation_id, .. }
        | &RasterDEMTileMessage::Error { correlation_id, .. }) = &message;
        let reply = encode_raster_reply(self.tile, correlation_id, &message, |error| RasterDEMTileMessage::Error {
            error: Error::native(error),
            correlation_id,
        });
        (self.replies)(reply);
    }
}
