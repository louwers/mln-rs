// Port-only: tile workers in other worker instances, reached only through
// bytes (mod.rs).
//
// `RemoteWorkers` is the map side. Each instance is a lane of `TileWorkers`:
// a job crosses as its frame through the lane's `MapCodec`, whose shared
// tables mirror the instance's, and each lane takes two jobs at a time.
// Each instance runs a `TileWorkerHost` (host.rs).
//
// An instance is lost when it reports its own death (a Web Worker's `error`
// event) or sends a frame whose header can't be read. Its lane's tables then
// start afresh on both sides, with a new instance in its place if the
// `WorkerInstances` can make one, and its tiles submit their jobs again.
//
// In the browser each instance is a Web Worker with its own wasm instance
// and memory. `ThreadWorkerInstances` runs the instances on threads of this
// process instead, each host on its thread and nothing shared with the map
// but the bytes; the render-test runner uses it
// (`--tileWorkerCodec`) to run the suites through the codec.

use std::sync::{Arc, mpsc};

use mln_style::hash::HashSet;
use mln_util::thread::Joined;
use mln_wire::payload::Payload;

use super::TileWorkerHost;
use super::envelope::{self, Encoded};
use super::map_side::MapCodec;
use crate::tile::inbox::{InboxSender, TileEvent};
use crate::tile::job::{JobId, JobResult, JobTimes, Run};
use crate::tile::tile_workers::{Lane, ReplyBody, TileWorkers};
use crate::{Registry, log_target};

/// The jobs a lane takes at once.
const JOBS_PER_LANE: usize = 2;

/// The worker instances a `RemoteWorkers` sends its frames to. Their
/// replies come back through the `ReplySink` the transport gave whoever
/// created them.
pub trait WorkerInstances {
    /// How many there are; frames go to `0..count()`.
    fn count(&self) -> usize;
    /// Sends a frame to the host of instance `instance`.
    fn post(&mut self, instance: usize, request: Vec<u8>);
    /// Puts a new instance, with a new host, in the place of `instance`,
    /// which was lost; its replies no longer arrive. Returns whether it
    /// could.
    fn replace(&mut self, _instance: usize) -> bool {
        false
    }
}

/// Delivers worker instances' replies, from any thread, to the inbox of the
/// tile store whose `RemoteWorkers` started them, in the order they were
/// sent. The store hands them to its workers when it takes in what arrived.
#[derive(Clone)]
pub struct ReplySink {
    inbox: InboxSender,
    origin: Origin,
}

impl ReplySink {
    /// Delivers a reply of instance `instance`.
    pub fn send(&self, instance: usize, reply: Encoded) {
        self.deliver(instance, Some(reply));
    }

    /// Reports that instance `instance` is gone, with every job it had.
    pub fn lost(&self, instance: usize) {
        self.deliver(instance, None);
    }

    fn deliver(&self, instance: usize, reply: Option<Encoded>) {
        let event = RemoteEvent { origin: self.origin.clone(), lane: Lane(instance), reply };
        self.inbox.send(TileEvent::Remote(event));
    }
}

/// Which `RemoteWorkers` a [`RemoteEvent`] is for: a store's inbox may still
/// hold events of the workers it replaced.
#[derive(Clone)]
struct Origin(Arc<()>);

impl Origin {
    fn is(&self, other: &Origin) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// What a worker instance sent its `RemoteWorkers`: a reply, or `None` when
/// the instance is lost.
pub(crate) struct RemoteEvent {
    origin: Origin,
    lane: Lane,
    reply: Option<Encoded>,
}

/// Makes the worker instances, given the sink for their replies.
type Start = Box<dyn FnOnce(ReplySink) -> Box<dyn WorkerInstances>>;

/// The map side of tile workers in other worker instances. The renderer
/// given it owns it (`Renderer::set_tile_workers`), and its instances start
/// then.
pub struct RemoteWorkers {
    registry: Arc<Registry>,
    origin: Origin,
    /// What makes the instances, until they start.
    start: Option<Start>,
    /// The instances, once started.
    instances: Option<Box<dyn WorkerInstances>>,
    /// Each instance's jobs; none until the instances start.
    lanes: Vec<RemoteLane>,
    /// The bytes sent and received for jobs.
    bytes: (u64, u64),
}

/// One instance's jobs.
struct RemoteLane {
    codec: MapCodec,
    in_flight: HashSet<JobId>,
    /// Counts the instances the lane had: a reply of an earlier one can't be
    /// read with this one's tables.
    instance: u64,
    /// Whether the lane's instance takes jobs: not once lost without a
    /// replacement.
    live: bool,
}

impl RemoteLane {
    /// The lane of its `instance`th instance, with tables of its own.
    fn new(registry: &Arc<Registry>, instance: u64, live: bool) -> Self {
        RemoteLane { codec: MapCodec::new(Arc::clone(registry)), in_flight: HashSet::default(), instance, live }
    }
}

impl RemoteWorkers {
    /// Workers in the instances that `instances` makes, given the sink for
    /// their replies, once a renderer's tiles take the workers. It must make
    /// at least one. The jobs name `registry`'s registrations, so each
    /// instance's host needs a registry filled by the same calls.
    pub fn new(
        registry: Arc<Registry>,
        instances: impl FnOnce(ReplySink) -> Box<dyn WorkerInstances> + 'static,
    ) -> Self {
        RemoteWorkers {
            registry,
            origin: Origin(Arc::new(())),
            start: Some(Box::new(instances)),
            instances: None,
            lanes: Vec::new(),
            bytes: (0, 0),
        }
    }

    fn job_reply(
        &mut self,
        lane: Lane,
        id: JobId,
        times: JobTimes,
        body: &[u8],
        payloads: Vec<Payload>,
    ) -> Option<TileEvent> {
        // A reply of an instance lost since isn't in flight any more.
        let remote = self.lanes.get_mut(lane.0)?;
        if !remote.in_flight.remove(&id) {
            return None;
        }
        let size = body.len() + payloads.iter().map(Payload::len).sum::<usize>();
        self.bytes.1 += size as u64;
        let body = ReplyBody::Encoded { body: Encoded { bytes: body.to_vec(), payloads }, instance: remote.instance };
        Some(TileEvent::Reply { lane, id, body, times })
    }

    /// Gives up on the instance of `lane`: its jobs, the replies not yet
    /// read, and its shared tables, on both sides.
    fn lose(&mut self, lane: Lane) -> Option<TileEvent> {
        let remote = self.lanes.get_mut(lane.0)?;
        log::error!(target: log_target::GENERAL, "Tile worker {} was lost", lane.0);
        let live = self.instances.as_mut().is_some_and(|instances| instances.replace(lane.0));
        *remote = RemoteLane::new(&self.registry, remote.instance + 1, live);
        Some(TileEvent::LaneLost(lane))
    }
}

impl TileWorkers for RemoteWorkers {
    fn connect(&mut self, inbox: &InboxSender) {
        let Some(start) = self.start.take() else {
            return;
        };
        let instances = start(ReplySink { inbox: inbox.clone(), origin: self.origin.clone() });
        let count = instances.count();
        assert!(count > 0, "RemoteWorkers need a worker instance");
        self.lanes = (0..count).map(|_| RemoteLane::new(&self.registry, 0, true)).collect();
        self.instances = Some(instances);
    }

    fn lanes(&self) -> usize {
        self.lanes.len()
    }

    fn capacity(&self, lane: Lane) -> usize {
        if self.lanes.get(lane.0).is_some_and(|remote| remote.live) { JOBS_PER_LANE } else { 0 }
    }

    fn caches_data(&self) -> bool {
        true
    }

    fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender) {
        let (Some(remote), Some(instances)) = (self.lanes.get_mut(lane.0), self.instances.as_mut()) else {
            return;
        };
        match remote.codec.encode_run(&run) {
            Ok(request) => {
                remote.in_flight.insert(run.id);
                self.bytes.0 += request.len() as u64;
                instances.post(lane.0, request);
            }
            // As a worker reports a job it can't decode.
            Err(error) => {
                let body = ReplyBody::Result(JobResult::Failed(mln_style::error::Error::native(error)));
                inbox.send(TileEvent::Reply { lane, id: run.id, body, times: JobTimes::default() });
            }
        }
    }

    fn receive(&mut self, event: RemoteEvent) -> Option<TileEvent> {
        if !event.origin.is(&self.origin) {
            return None;
        }
        let lane = event.lane;
        let Some(reply) = event.reply else {
            return self.lose(lane);
        };
        match envelope::read_reply_header(&reply.bytes) {
            Some((id, times, body)) => self.job_reply(lane, id, times, body, reply.payloads),
            None => self.lose(lane),
        }
    }

    fn decode(&mut self, lane: Lane, body: ReplyBody) -> Option<JobResult> {
        match body {
            ReplyBody::Result(result) => Some(result),
            ReplyBody::Encoded { body, instance } => {
                let remote = self.lanes.get_mut(lane.0).filter(|remote| remote.instance == instance)?;
                Some(remote.codec.decode_result(body))
            }
        }
    }

    fn bytes(&self) -> (u64, u64) {
        self.bytes
    }
}

/// Worker instances on threads of this process: a `TileWorkerHost` on each
/// thread.
pub struct ThreadWorkerInstances {
    /// Each instance's thread, or `None` for one that couldn't start, whose
    /// lane is lost.
    threads: Vec<Option<WorkerThread>>,
    registry: Arc<Registry>,
    replies: ReplySink,
}

/// A thread that builds a `TileWorkerHost` and hands it the requests sent
/// to it, in order. Dropping it lets the thread end once it handled the
/// requests sent before, and waits for it.
struct WorkerThread {
    requests: mpsc::Sender<Vec<u8>>,
    _thread: Joined,
}

impl ThreadWorkerInstances {
    /// `count` instances, whose hosts resolve their jobs' registrations
    /// against `registry` and send their replies to `replies`.
    #[must_use]
    pub fn new(count: usize, registry: Arc<Registry>, replies: &ReplySink) -> Self {
        let threads = (0..count)
            .map(|instance| {
                let thread = Self::start(instance, &registry, replies);
                if thread.is_none() {
                    replies.lost(instance);
                }
                thread
            })
            .collect();
        ThreadWorkerInstances { threads, registry, replies: replies.clone() }
    }

    fn start(instance: usize, registry: &Arc<Registry>, replies: &ReplySink) -> Option<WorkerThread> {
        let (registry, replies) = (Arc::clone(registry), replies.clone());
        let (requests, received) = mpsc::channel::<Vec<u8>>();
        let thread = mln_util::thread::spawn(format!("TileWorker {instance}"), move || {
            let host = TileWorkerHost::new(registry, move |reply| replies.send(instance, reply));
            for request in received {
                host.handle(&request);
            }
        });
        match thread {
            Ok(thread) => Some(WorkerThread { requests, _thread: Joined::new(thread) }),
            Err(error) => {
                log::error!(target: log_target::GENERAL, "Tile worker {instance}'s thread didn't start: {error}");
                None
            }
        }
    }
}

impl WorkerInstances for ThreadWorkerInstances {
    fn count(&self) -> usize {
        self.threads.len()
    }

    fn post(&mut self, instance: usize, request: Vec<u8>) {
        if let Some(thread) = &self.threads[instance] {
            let _ = thread.requests.send(request);
        }
    }

    fn replace(&mut self, instance: usize) -> bool {
        self.threads[instance] = Self::start(instance, &self.registry, &self.replies);
        self.threads[instance].is_some()
    }
}
