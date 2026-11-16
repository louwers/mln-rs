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

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mln_actor::{LocalObject, SchedulerPtr, Thread};
use mln_util::hash::HashSet;
use mln_util::logging::{Event, Log};
use mln_util::time::Instant;

use super::TileWorkerHost;
use super::envelope::{self, Encoded};
use super::map_side::MapCodec;
use crate::Registry;
use crate::tile::inbox::{InboxSender, TileEvent};
use crate::tile::job::{JobId, JobResult, JobTimes, Run};
use crate::tile::tile_workers::{Lane, ReplyBody, TileWorkers};

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

/// Delivers worker instances' replies to their `RemoteWorkers`, from any
/// thread: each is handled by a task on the transport's thread, in the
/// order they were sent.
#[derive(Clone)]
pub struct ReplySink {
    scheduler: SchedulerPtr,
    workers: Arc<LocalObject<Weak<RemoteWorkers>>>,
}

impl ReplySink {
    /// Delivers a reply of instance `instance`.
    pub fn send(&self, instance: usize, reply: Encoded) {
        self.deliver(move |workers| workers.receive(instance, reply));
    }

    /// Reports that instance `instance` is gone, with every job it had.
    pub fn lost(&self, instance: usize) {
        self.deliver(move |workers| workers.lose(Lane(instance)));
    }

    fn deliver(&self, f: impl FnOnce(&RemoteWorkers) + Send + 'static) {
        let workers = Arc::clone(&self.workers);
        self.scheduler.schedule(Box::new(move || {
            if let Some(workers) = workers.get().and_then(|weak| weak.upgrade()) {
                f(&workers);
            }
        }));
    }
}

/// The map side of tile workers in other worker instances.
pub struct RemoteWorkers {
    state: RefCell<State>,
    /// Whether each lane's instance takes jobs: not once lost without a
    /// replacement.
    live: Vec<Cell<bool>>,
    /// Where the replies go: the inbox of the first renderer that submits.
    inbox: OnceCell<InboxSender>,
    /// The bytes sent and received for jobs.
    bytes: Cell<(u64, u64)>,
    /// The time this thread spent on replies since `take_receive_time`.
    receive_time: Cell<Duration>,
}

struct State {
    instances: Box<dyn WorkerInstances>,
    lanes: Vec<RemoteLane>,
    /// What the jobs name.
    registry: Arc<Registry>,
}

/// One instance's jobs.
struct RemoteLane {
    codec: MapCodec,
    in_flight: HashSet<JobId>,
    /// Counts the instances the lane had: a reply of an earlier one can't be
    /// read with this one's tables.
    instance: u64,
}

impl RemoteLane {
    /// The lane of its `instance`th instance, with tables of its own.
    fn new(registry: &Arc<Registry>, instance: u64) -> Self {
        RemoteLane { codec: MapCodec::new(Arc::clone(registry)), in_flight: HashSet::default(), instance }
    }
}

impl RemoteWorkers {
    /// Workers in the instances `instances` creates, with the sink for their
    /// replies. The replies are handled in tasks of `scheduler`, the
    /// renderer's loop, which runs on this thread, the one that uses the
    /// workers. The jobs name `registry`'s registrations, so each instance's
    /// host needs a registry filled by the same calls.
    pub fn new(
        scheduler: &SchedulerPtr,
        registry: Arc<Registry>,
        instances: impl FnOnce(ReplySink) -> Box<dyn WorkerInstances>,
    ) -> Rc<Self> {
        Rc::new_cyclic(|self_weak: &Weak<RemoteWorkers>| {
            let workers = Arc::new(LocalObject::new(Weak::clone(self_weak)));
            let instances = instances(ReplySink { scheduler: Arc::clone(scheduler), workers });
            let count = instances.count();
            assert!(count > 0, "RemoteWorkers need a worker instance");
            RemoteWorkers {
                state: RefCell::new(State {
                    instances,
                    lanes: (0..count).map(|_| RemoteLane::new(&registry, 0)).collect(),
                    registry,
                }),
                live: (0..count).map(|_| Cell::new(true)).collect(),
                inbox: OnceCell::new(),
                bytes: Cell::new((0, 0)),
                receive_time: Cell::new(Duration::ZERO),
            }
        })
    }

    /// Port-only: the time this thread spent receiving replies and decoding
    /// them, since the last call.
    pub fn take_receive_time(&self) -> Duration {
        self.receive_time.take()
    }

    fn send(&self, event: TileEvent) {
        if let Some(inbox) = self.inbox.get() {
            inbox.send(event);
        }
    }

    fn receive(&self, instance: usize, reply: Encoded) {
        let start = Instant::now();
        match envelope::read_reply_header(&reply.bytes) {
            Some((id, times, body)) => self.job_reply(Lane(instance), id, times, body, reply.payloads),
            None => self.lose(Lane(instance)),
        }
        self.receive_time.set(self.receive_time.get() + start.elapsed());
    }

    fn job_reply(
        &self,
        lane: Lane,
        id: JobId,
        times: JobTimes,
        body: &[u8],
        payloads: Vec<mln_util::serialization::payload::Payload>,
    ) {
        let body = {
            let mut state = self.state.borrow_mut();
            // A reply of an instance lost since isn't in flight any more.
            let Some(remote) = state.lanes.get_mut(lane.0) else {
                return;
            };
            if !remote.in_flight.remove(&id) {
                return;
            }
            let size = body.len() + payloads.iter().map(mln_util::serialization::payload::Payload::len).sum::<usize>();
            let (sent, received) = self.bytes.get();
            self.bytes.set((sent, received + size as u64));
            ReplyBody::Encoded { body: Encoded { bytes: body.to_vec(), payloads }, instance: remote.instance }
        };
        self.send(TileEvent::Reply { lane, id, body, times });
    }

    /// Gives up on the instance of `lane`: its jobs, the replies not yet
    /// read, and its shared tables, on both sides.
    fn lose(&self, lane: Lane) {
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let (Some(remote), Some(live)) = (state.lanes.get_mut(lane.0), self.live.get(lane.0)) else {
                return;
            };
            Log::error(Event::General, format!("Tile worker {} was lost", lane.0));
            live.set(state.instances.replace(lane.0));
            *remote = RemoteLane::new(&state.registry, remote.instance + 1);
        }
        self.send(TileEvent::LaneLost(lane));
    }
}

/// The renderer's handle on `RemoteWorkers`, as its `TileWorkers`.
pub(crate) struct RemoteJobs(pub(crate) Rc<RemoteWorkers>);

impl TileWorkers for RemoteJobs {
    fn lanes(&self) -> usize {
        self.0.live.len()
    }

    fn capacity(&self, lane: Lane) -> usize {
        if self.0.live.get(lane.0).is_some_and(Cell::get) { JOBS_PER_LANE } else { 0 }
    }

    fn caches_data(&self) -> bool {
        true
    }

    fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender) {
        let _ = self.0.inbox.set(inbox.clone());
        let failed = {
            let mut state = self.0.state.borrow_mut();
            let state = &mut *state;
            let Some(remote) = state.lanes.get_mut(lane.0) else {
                return;
            };
            match remote.codec.encode_run(&run) {
                Ok(request) => {
                    remote.in_flight.insert(run.id);
                    let (sent, received) = self.0.bytes.get();
                    self.0.bytes.set((sent + request.len() as u64, received));
                    state.instances.post(lane.0, request);
                    None
                }
                Err(error) => Some(error),
            }
        };
        // As a worker reports a job it can't decode.
        if let Some(error) = failed {
            let body = ReplyBody::Result(JobResult::Failed(mln_util::error::Error::native(error)));
            self.0.send(TileEvent::Reply { lane, id: run.id, body, times: JobTimes::default() });
        }
    }

    fn decode(&mut self, lane: Lane, body: ReplyBody) -> Option<JobResult> {
        match body {
            ReplyBody::Result(result) => Some(result),
            ReplyBody::Encoded { body, instance } => {
                let start = Instant::now();
                let mut state = self.0.state.borrow_mut();
                let remote = state.lanes.get_mut(lane.0).filter(|remote| remote.instance == instance)?;
                let result = remote.codec.decode_result(body);
                self.0.receive_time.set(self.0.receive_time.get() + start.elapsed());
                Some(result)
            }
        }
    }

    fn bytes(&self) -> (u64, u64) {
        self.0.bytes.get()
    }
}

/// Worker instances on threads of this process: a `TileWorkerHost` on each
/// thread.
pub struct ThreadWorkerInstances {
    threads: Vec<Thread<TileWorkerHost>>,
    registry: Arc<Registry>,
    replies: ReplySink,
}

impl ThreadWorkerInstances {
    /// `count` instances, whose hosts resolve their jobs' registrations
    /// against `registry` and send their replies to `replies`.
    #[must_use]
    pub fn new(count: usize, registry: Arc<Registry>, replies: &ReplySink) -> Self {
        let threads = (0..count).map(|instance| Self::start(instance, &registry, replies)).collect();
        ThreadWorkerInstances { threads, registry, replies: replies.clone() }
    }

    fn start(instance: usize, registry: &Arc<Registry>, replies: &ReplySink) -> Thread<TileWorkerHost> {
        let (registry, replies) = (Arc::clone(registry), replies.clone());
        Thread::new(&format!("TileWorker {instance}"), move |_, _| {
            TileWorkerHost::new(registry, move |reply| replies.send(instance, reply))
        })
    }
}

impl WorkerInstances for ThreadWorkerInstances {
    fn count(&self) -> usize {
        self.threads.len()
    }

    fn post(&mut self, instance: usize, request: Vec<u8>) {
        self.threads[instance].actor().invoke(move |host: &TileWorkerHost| host.handle(&request));
    }

    fn replace(&mut self, instance: usize) -> bool {
        self.threads[instance] = Self::start(instance, &self.registry, &self.replies);
        true
    }
}
