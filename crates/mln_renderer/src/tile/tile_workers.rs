//! Where tiles' jobs run: the map side of the tile workers, behind one
//! trait, [`TileWorkers`], with three transports. [`PoolWorkers`] runs jobs
//! on the renderer's thread pool, [`InlineWorkers`] runs each inside
//! `submit` (sources with synchronous updates, and tests), and
//! `RemoteWorkers` (`worker_codec`) in other wasm instances, through bytes.
//! [`TileJobs`] is the renderer's side of them: it bounds the jobs in flight
//! on each lane, keeps the replies until their tiles take them, and
//! remembers which lane last got each tile's data.

use std::cell::Cell;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use mln_actor::TaggedScheduler;
use mln_util::hash::HashMap;
use mln_util::logging::{Event, EventSeverity, Log};

use crate::tile::job::{DataKey, DiscoveryMemo, JobId, JobResult, JobTimes, Reply, Run, SourceKey, run_job};
use crate::tile::worker_codec::Encoded;

/// One worker the jobs go to: one Web Worker, or the whole thread pool.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Lane(pub(crate) usize);

/// Asks the renderer for an update, from any thread.
pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

/// What comes back from the workers.
#[cfg_attr(
    target_pointer_width = "64",
    expect(clippy::large_enum_variant, reason = "few events live at once, and briefly")
)]
pub(crate) enum WorkerEvent {
    Reply {
        lane: Lane,
        id: JobId,
        body: ReplyBody,
        times: JobTimes,
    },
    /// The worker of `lane` is gone, with every job it had: its replies
    /// never come.
    LaneLost(Lane),
}

/// A reply's result, or its bytes, which only a reply its tile takes is
/// decoded from.
pub(crate) enum ReplyBody {
    Result(JobResult),
    Encoded(Encoded),
}

/// The map side of the tile workers.
pub(crate) trait TileWorkers {
    fn lanes(&self) -> usize;
    /// The jobs submitted to `lane` whose replies haven't been received.
    fn in_flight(&self, lane: Lane) -> usize;
    /// The most jobs `lane` takes at once: none for a lane that is gone.
    fn capacity(&self, lane: Lane) -> usize;
    /// Whether the workers keep tile data, which a job can name instead of
    /// carrying (`DataInput::Cached`).
    fn caches_data(&self) -> bool {
        false
    }
    /// Starts `run` on `lane`; its reply comes from `receive`.
    fn submit(&mut self, lane: Lane, run: Run);
    /// Adds the replies and lost lanes that arrived since the last call.
    fn receive(&mut self, events: &mut Vec<WorkerEvent>);
    /// The result of a reply of `lane`.
    fn decode(&mut self, _lane: Lane, body: ReplyBody) -> JobResult {
        match body {
            ReplyBody::Result(result) => result,
            ReplyBody::Encoded(_) => JobResult::Failed(mln_util::error::Error::native("an encoded reply")),
        }
    }
    /// The bytes sent to the workers and received from them so far.
    fn bytes(&self) -> (u64, u64) {
        (0, 0)
    }
}

/// The jobs whose discovery `PoolWorkers` keep for their resume.
const MEMO_JOBS: usize = 64;

/// The renderer's thread pool, as one lane with one queue.
pub(crate) struct PoolWorkers {
    pool: TaggedScheduler,
    memo: Arc<DiscoveryMemo>,
    capacity: usize,
    in_flight: usize,
    sender: mpsc::Sender<(Reply, JobTimes)>,
    replies: mpsc::Receiver<(Reply, JobTimes)>,
    wake: Waker,
}

impl PoolWorkers {
    /// Workers on `pool` that take `capacity` jobs at once and call `wake`
    /// as each reply arrives.
    pub(crate) fn new(pool: TaggedScheduler, capacity: usize, wake: Waker) -> Self {
        let (sender, replies) = mpsc::channel();
        // Enough for the labelled tiles of a view that wait for glyphs.
        let memo = Arc::new(DiscoveryMemo::new(MEMO_JOBS));
        PoolWorkers { pool, memo, capacity, in_flight: 0, sender, replies, wake }
    }
}

impl TileWorkers for PoolWorkers {
    fn lanes(&self) -> usize {
        1
    }

    fn in_flight(&self, _lane: Lane) -> usize {
        self.in_flight
    }

    fn capacity(&self, _lane: Lane) -> usize {
        self.capacity
    }

    fn submit(&mut self, _lane: Lane, run: Run) {
        self.in_flight += 1;
        let memo = Arc::clone(&self.memo);
        let sender = self.sender.clone();
        let wake = Arc::clone(&self.wake);
        self.pool.schedule(Box::new(move || {
            let done = move |reply, times| {
                // The renderer may be gone, and its replies with it.
                let _ = sender.send((reply, times));
                wake();
            };
            if run.superseded.load(Ordering::Relaxed) {
                let result = JobResult::Failed(mln_util::error::Error::native("superseded"));
                return done(Reply { id: run.id, result }, JobTimes::default());
            }
            run_job(run, None, Some(&memo), done);
        }));
    }

    fn receive(&mut self, events: &mut Vec<WorkerEvent>) {
        while let Ok((Reply { id, result }, times)) = self.replies.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            events.push(WorkerEvent::Reply { lane: Lane(0), id, body: ReplyBody::Result(result), times });
        }
    }
}

/// Runs each job inside `submit`, on the calling thread. Natively every job
/// replies before `submit` returns; on the web, an image decode replies
/// later.
pub(crate) struct InlineWorkers {
    sender: mpsc::Sender<(Reply, JobTimes)>,
    replies: mpsc::Receiver<(Reply, JobTimes)>,
    in_flight: usize,
}

impl Default for InlineWorkers {
    fn default() -> Self {
        let (sender, replies) = mpsc::channel();
        InlineWorkers { sender, replies, in_flight: 0 }
    }
}

impl InlineWorkers {
    /// Runs `run` now and returns its reply. On the web, an image decode
    /// answers later: its reply here is `Failed`.
    pub(crate) fn run(run: Run) -> Reply {
        let id = run.id;
        let (sender, reply) = mpsc::channel();
        run_job(run, None, None, move |reply, _| {
            let _ = sender.send(reply);
        });
        reply
            .try_recv()
            .unwrap_or_else(|_| Reply { id, result: JobResult::Failed(mln_util::error::Error::native("no reply")) })
    }
}

impl TileWorkers for InlineWorkers {
    fn lanes(&self) -> usize {
        1
    }

    fn in_flight(&self, _lane: Lane) -> usize {
        self.in_flight
    }

    fn capacity(&self, _lane: Lane) -> usize {
        usize::MAX
    }

    fn submit(&mut self, _lane: Lane, run: Run) {
        self.in_flight += 1;
        let sender = self.sender.clone();
        run_job(run, None, None, move |reply, times| {
            let _ = sender.send((reply, times));
        });
    }

    fn receive(&mut self, events: &mut Vec<WorkerEvent>) {
        while let Ok((Reply { id, result }, times)) = self.replies.try_recv() {
            self.in_flight -= 1;
            events.push(WorkerEvent::Reply { lane: Lane(0), id, body: ReplyBody::Result(result), times });
        }
    }
}

/// A source's counters, which its tiles take their generations and the keys
/// of their data loads from, so that no two of its tiles' jobs or loads
/// share one.
pub(crate) struct SourceCounters {
    key: SourceKey,
    generation: Cell<u64>,
    load: Cell<u64>,
}

impl SourceCounters {
    pub(crate) fn new(key: SourceKey) -> Self {
        SourceCounters { key, generation: Cell::new(0), load: Cell::new(0) }
    }

    pub(crate) fn key(&self) -> SourceKey {
        self.key
    }

    pub(crate) fn next_generation(&self) -> u64 {
        self.generation.set(self.generation.get() + 1);
        self.generation.get()
    }

    pub(crate) fn next_load(&self) -> u64 {
        self.load.set(self.load.get() + 1);
        self.load.get()
    }
}

/// Whether `lane` takes another job.
fn has_room(workers: &dyn TileWorkers, lane: Lane) -> bool {
    workers.in_flight(lane) < workers.capacity(lane)
}

/// The most data keys whose lane `TileJobs` remembers; past it, it forgets
/// them all, which costs only the data crossing again.
const MAX_HINTS: usize = 4096;

/// The renderer's side of its tile workers, for one update at a time: the
/// replies `receive` took until their tiles take them, the lanes lost, and
/// where each tile's data last went.
pub(crate) struct TileJobs {
    workers: Box<dyn TileWorkers>,
    replies: HashMap<JobId, (Lane, ReplyBody)>,
    lost: Vec<Lane>,
    hints: HashMap<DataKey, Lane>,
    stats: JobStats,
}

/// What the jobs did, for the debug log.
#[derive(Default)]
struct JobStats {
    submitted: u64,
    received: u64,
    needs_deps: u64,
    needs_data: u64,
    discover: Duration,
    build: Duration,
}

impl TileJobs {
    pub(crate) fn new(workers: Box<dyn TileWorkers>) -> Self {
        TileJobs {
            workers,
            replies: HashMap::default(),
            lost: Vec::new(),
            hints: HashMap::default(),
            stats: JobStats::default(),
        }
    }

    /// Replaces the workers. The jobs in flight on the previous ones are
    /// lost: their tiles submit them again.
    pub(crate) fn set_workers(&mut self, workers: Box<dyn TileWorkers>) {
        self.lost.extend((0..self.workers.lanes()).map(Lane));
        self.workers = workers;
        self.hints.clear();
    }

    /// Takes the replies and lost lanes that arrived.
    pub(crate) fn receive(&mut self) {
        let mut events = Vec::new();
        self.workers.receive(&mut events);
        for event in events {
            match event {
                WorkerEvent::Reply { lane, id, body, times } => {
                    self.stats.received += 1;
                    self.stats.discover += times.discover;
                    self.stats.build += times.build;
                    self.replies.insert(id, (lane, body));
                }
                WorkerEvent::LaneLost(lane) => {
                    self.hints.retain(|_, hint| *hint != lane);
                    self.lost.push(lane);
                }
            }
        }
    }

    /// Whether no job is in flight and no reply waits for its tile.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.replies.is_empty() && (0..self.workers.lanes()).all(|lane| self.workers.in_flight(Lane(lane)) == 0)
    }

    /// Whether the reply to job `id` arrived.
    pub(crate) fn has_reply(&self, id: &JobId) -> bool {
        self.replies.contains_key(id)
    }

    /// The result of job `id`, if its reply arrived.
    pub(crate) fn take_reply(&mut self, id: &JobId) -> Option<JobResult> {
        let (lane, body) = self.replies.remove(id)?;
        let result = self.workers.decode(lane, body);
        match result {
            JobResult::NeedsDeps(_) => self.stats.needs_deps += 1,
            JobResult::NeedsData => self.stats.needs_data += 1,
            JobResult::Laid(_) | JobResult::Raster(_) | JobResult::Dem(_) | JobResult::Failed(_) => {}
        }
        Some(result)
    }

    /// Whether `lane` was lost since the last `end_receive`.
    pub(crate) fn is_lost(&self, lane: Lane) -> bool {
        self.lost.contains(&lane)
    }

    /// Ends the tiles' turn to take their replies: returns those no tile
    /// took, which are stale, and forgets the lost lanes.
    pub(crate) fn end_receive(&mut self) -> Vec<ReplyBody> {
        self.lost.clear();
        self.replies.drain().map(|(_, (_, body))| body).collect()
    }

    /// The lane for a job on data `key`, and whether the job can name the
    /// data rather than carry it: the lane that last got the data while it
    /// has room, or else the least loaded lane with room, which then becomes
    /// the data's lane. `None` if no lane has room. `full` asks for a lane
    /// that gets the data.
    pub(crate) fn place(&mut self, key: DataKey, full: bool) -> Option<(Lane, bool)> {
        let workers = &*self.workers;
        if workers.caches_data()
            && !full
            && let Some(&lane) = self.hints.get(&key)
            && has_room(workers, lane)
        {
            return Some((lane, true));
        }
        let lane = self.lane()?;
        if !self.workers.caches_data() {
            return Some((lane, false));
        }
        if self.hints.len() >= MAX_HINTS {
            self.hints.clear();
        }
        self.hints.insert(key, lane);
        Some((lane, false))
    }

    /// The least loaded lane with room, for a job that carries all it
    /// reads; `None` if no lane has room.
    pub(crate) fn lane(&self) -> Option<Lane> {
        let workers = &*self.workers;
        (0..workers.lanes())
            .map(Lane)
            .filter(|lane| has_room(workers, *lane))
            .min_by_key(|lane| workers.in_flight(*lane))
    }

    pub(crate) fn submit(&mut self, lane: Lane, run: Run) {
        self.stats.submitted += 1;
        self.workers.submit(lane, run);
    }

    /// Logs what the jobs did, at debug level.
    pub(crate) fn log_stats(&self) {
        let JobStats { submitted, received, needs_deps, needs_data, discover, build } = self.stats;
        if submitted == 0 {
            return;
        }
        let (sent, got) = self.workers.bytes();
        let share = |part: u64, whole: u64| if whole == 0 { 0.0 } else { part as f64 * 100.0 / whole as f64 };
        let layout = discover + build;
        let discover_share = if layout.is_zero() { 0.0 } else { discover.as_secs_f64() * 100.0 / layout.as_secs_f64() };
        Log::record(
            EventSeverity::Debug,
            Event::Timing,
            -1,
            &format!(
                "tile jobs: {submitted} submitted, {received} replied, {:.1}% NeedsDeps, {:.1}% NeedsData, \
                 discovery {discover_share:.1}% of {:.1} ms layout, {} B sent and {} B received per job",
                share(needs_deps, received),
                share(needs_data, received),
                layout.as_secs_f64() * 1000.0,
                sent / submitted,
                got / received.max(1),
            ),
        );
    }
}
