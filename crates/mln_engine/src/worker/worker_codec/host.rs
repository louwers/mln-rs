// Port-only: what one worker instance runs, behind bytes (mod.rs).
//
// A `TileWorkerHost` decodes the map side's frames. It runs each job as its
// frame arrives, one job per frame, and sends the reply back once the job
// answers: a layout before `handle` returns, an image decode when the
// platform's decoder answers, which in the browser is later. It keeps the
// tile data of the jobs in a cache of its own, which a later job can name
// instead of carrying the data. A job whose body it can't decode gets a
// `Failed` reply with the header's id, and a frame whose header it can't
// read an empty reply, which the map side takes as the instance lost.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_style::error::Error;

use super::envelope::{self, Encoded};
use super::worker_side::WorkerCodec;
use crate::worker::job::{
    DEFAULT_DATA_CACHE_BYTES, DataCache, DiscoveryMemo, JobResult, JobTimes, Reply, Run, run_job,
};
use crate::{Registry, log_target};

/// Where a host's replies go, encoded: back to the map side of its
/// instance. Their payloads (pixels, vertices, indices) can be transferred
/// there rather than copied.
pub(crate) type Replies = Arc<dyn Fn(Encoded) + Send + Sync>;

/// What one worker instance runs.
pub struct TileWorkerHost {
    jobs: RefCell<Jobs>,
    /// Shared with the replies of image decodes, which encode with it when
    /// the decoder answers.
    codec: Rc<RefCell<WorkerCodec>>,
    replies: Replies,
}

/// The jobs' side of an instance.
struct Jobs {
    cache: DataCache,
    memo: DiscoveryMemo,
}

impl TileWorkerHost {
    /// A host that resolves the registrations its jobs name against
    /// `registry` and sends its replies to `replies`. `registry` must be
    /// filled by the same calls as the map side's. Its data cache keeps
    /// 32 MiB.
    pub fn new(registry: Arc<Registry>, replies: impl Fn(Encoded) + Send + Sync + 'static) -> Self {
        TileWorkerHost {
            jobs: RefCell::new(Jobs {
                cache: DataCache::new(DEFAULT_DATA_CACHE_BYTES),
                // The labelled tiles of a view that resume here, about.
                memo: DiscoveryMemo::new(16),
            }),
            codec: Rc::new(RefCell::new(WorkerCodec::new(registry))),
            replies: Arc::new(replies),
        }
    }

    /// Runs the job in a frame from the map side, and sends its reply.
    pub fn handle(&self, request: &[u8]) {
        let Some((id, body)) = envelope::read_job_id(request) else {
            return self.lost("a tile worker request without a job id");
        };
        let decoded = self.codec.borrow_mut().decode_job(body);
        let send = {
            let codec = Rc::clone(&self.codec);
            let replies = Arc::clone(&self.replies);
            move |reply: Reply, times| replies(codec.borrow_mut().encode_reply(&reply, &times))
        };
        match decoded {
            Ok(job) => {
                let mut jobs = self.jobs.borrow_mut();
                let Jobs { cache, memo } = &mut *jobs;
                run_job(Run { id, job, superseded: Arc::default() }, Some(cache), Some(memo), send);
            }
            Err(error) => {
                let result = JobResult::Failed(Error::native(format!("a tile worker request: {error}")));
                send(Reply { id, result }, JobTimes::default());
            }
        }
    }

    /// Tells the map side that this instance can't go on: a frame it can't
    /// read leaves it out of step with the map side's tables.
    fn lost(&self, error: &str) {
        log::error!(target: log_target::GENERAL, "Not a tile worker request: {error}");
        (self.replies)(Encoded::default());
    }
}
