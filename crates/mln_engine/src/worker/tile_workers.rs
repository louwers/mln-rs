//! Where tiles' jobs run: the map side of the tile workers, behind one
//! trait, [`TileWorkers`], with three transports. [`PoolWorkers`] runs jobs
//! on the renderer's thread pool, [`InlineWorkers`] runs each inside
//! `submit` (tests), and `RemoteWorkers` (`worker_codec`) in other wasm
//! instances, through bytes. Every transport sends its replies to the tile
//! store's inbox. [`Jobs`] is the store's side of them: it bounds the jobs
//! in flight on each lane, and remembers which lane last got each tile's
//! data.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use mln_style::hash::HashMap;

use crate::exec::pool::TaskGroup;
use crate::log_target;
use crate::tile_store::inbox::{InboxSender, TileEvent};
use crate::worker::job::{DataKey, DiscoveryMemo, JobId, JobResult, JobTimes, Reply, Run, run_job};
use crate::worker::worker_codec::{Encoded, RemoteEvent};

/// One worker the jobs go to: one Web Worker, or the whole thread pool.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Lane(pub(crate) usize);

/// A reply's result, or its bytes, which only a reply its tile takes is
/// decoded from.
pub(crate) enum ReplyBody {
    Result(JobResult),
    /// The bytes of a reply of a lane's worker instance, the `instance`th
    /// that lane had.
    Encoded {
        body: Encoded,
        instance: u64,
    },
}

/// The map side of the tile workers.
pub(crate) trait TileWorkers {
    /// Called once, before anything else, with the inbox of the store that
    /// takes the workers.
    fn connect(&mut self, _inbox: &InboxSender) {}
    fn lanes(&self) -> usize;
    /// The most jobs `lane` takes at once: none for a lane that is gone.
    fn capacity(&self, lane: Lane) -> usize;
    /// Whether the workers keep tile data, which a job can name instead of
    /// carrying (`DataInput::Cached`).
    fn caches_data(&self) -> bool {
        false
    }
    /// Starts `run` on `lane`; its reply, or the loss of the lane, goes to
    /// `inbox`.
    fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender);
    /// Reads what a remote worker instance sent: the reply or lost lane it
    /// makes, if any. Events of other workers make none.
    fn receive(&mut self, _event: RemoteEvent) -> Option<TileEvent> {
        None
    }
    /// The result of a reply of `lane`; `None` for one whose worker instance
    /// was lost since, which can't be read any more.
    fn decode(&mut self, _lane: Lane, body: ReplyBody) -> Option<JobResult> {
        match body {
            ReplyBody::Result(result) => Some(result),
            ReplyBody::Encoded { .. } => Some(JobResult::Failed(mln_style::error::Error::native("an encoded reply"))),
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
    pool: TaskGroup,
    memo: Arc<DiscoveryMemo>,
    capacity: usize,
}

impl PoolWorkers {
    /// Workers on `pool` that take `capacity` jobs at once.
    pub(crate) fn new(pool: TaskGroup, capacity: usize) -> Self {
        // Enough for the labelled tiles of a view that wait for glyphs.
        let memo = Arc::new(DiscoveryMemo::new(MEMO_JOBS));
        PoolWorkers { pool, memo, capacity }
    }
}

impl TileWorkers for PoolWorkers {
    fn lanes(&self) -> usize {
        1
    }

    fn capacity(&self, _lane: Lane) -> usize {
        self.capacity
    }

    fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender) {
        let memo = Arc::clone(&self.memo);
        let inbox = inbox.clone();
        self.pool.spawn(move || {
            let done = move |Reply { id, result }, times| {
                inbox.send(TileEvent::Reply { lane, id, body: ReplyBody::Result(result), times });
            };
            if run.superseded.load(Ordering::Relaxed) {
                let result = JobResult::Failed(mln_style::error::Error::native("superseded"));
                return done(Reply { id: run.id, result }, JobTimes::default());
            }
            run_job(run, None, Some(&memo), done);
        });
    }
}

/// Runs each job inside `submit`, on the calling thread. Natively every job
/// replies before `submit` returns; on the web, an image decode replies
/// later.
#[derive(Default)]
pub(crate) struct InlineWorkers;

impl InlineWorkers {
    /// Runs `run` now and returns its reply. On the web, an image decode
    /// answers later: its reply here is `Failed`.
    pub(crate) fn run(run: Run) -> Reply {
        let id = run.id;
        let (sender, reply) = std::sync::mpsc::channel();
        run_job(run, None, None, move |reply, _| {
            let _ = sender.send(reply);
        });
        reply
            .try_recv()
            .unwrap_or_else(|_| Reply { id, result: JobResult::Failed(mln_style::error::Error::native("no reply")) })
    }
}

impl TileWorkers for InlineWorkers {
    fn lanes(&self) -> usize {
        1
    }

    fn capacity(&self, _lane: Lane) -> usize {
        usize::MAX
    }

    fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender) {
        let inbox = inbox.clone();
        run_job(run, None, None, move |Reply { id, result }, times| {
            inbox.send(TileEvent::Reply { lane, id, body: ReplyBody::Result(result), times });
        });
    }
}

/// The most data keys whose lane `Jobs` remembers; past it, it forgets
/// them all, which costs only the data crossing again.
const MAX_HINTS: usize = 4096;

/// The tile store's side of its workers: the jobs in flight, by the lane
/// they went to, and where each tile's data last went.
pub(crate) struct Jobs {
    workers: Box<dyn TileWorkers>,
    in_flight: HashMap<JobId, Lane>,
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

impl Jobs {
    /// The jobs of `workers`, whose replies go to `inbox`.
    pub(crate) fn new(mut workers: Box<dyn TileWorkers>, inbox: &InboxSender) -> Self {
        workers.connect(inbox);
        Jobs { workers, in_flight: HashMap::default(), hints: HashMap::default(), stats: JobStats::default() }
    }

    /// Replaces the workers. The jobs in flight on the previous ones are
    /// lost: their replies, if they come, find no job of these workers.
    pub(crate) fn set_workers(&mut self, mut workers: Box<dyn TileWorkers>, inbox: &InboxSender) {
        workers.connect(inbox);
        self.workers = workers;
        self.in_flight.clear();
        self.hints.clear();
    }

    /// Whether no job is in flight.
    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
    }

    /// Takes note of the reply to job `id`; returns whether the job was in
    /// flight on these workers.
    pub(crate) fn replied(&mut self, id: &JobId, times: JobTimes) -> bool {
        if self.in_flight.remove(id).is_none() {
            return false;
        }
        self.stats.received += 1;
        self.stats.discover += times.discover;
        self.stats.build += times.build;
        true
    }

    /// Hands the workers what a remote worker instance sent; returns the
    /// reply or lost lane it makes, if any.
    pub(crate) fn receive(&mut self, event: RemoteEvent) -> Option<TileEvent> {
        self.workers.receive(event)
    }

    /// Forgets `lane`'s jobs and the data it had: its worker is gone.
    pub(crate) fn lost(&mut self, lane: Lane) {
        self.in_flight.retain(|_, in_flight| *in_flight != lane);
        self.hints.retain(|_, hint| *hint != lane);
    }

    /// The result of a reply of `lane`; `None` if it can't be read since its
    /// worker was lost.
    pub(crate) fn decode(&mut self, lane: Lane, body: ReplyBody) -> Option<JobResult> {
        let result = self.workers.decode(lane, body)?;
        match result {
            JobResult::NeedsDeps(_) => self.stats.needs_deps += 1,
            JobResult::NeedsData => self.stats.needs_data += 1,
            JobResult::Laid(_) | JobResult::Raster(_) | JobResult::Dem(_) | JobResult::Failed(_) => {}
        }
        Some(result)
    }

    fn in_flight(&self, lane: Lane) -> usize {
        self.in_flight.values().filter(|in_flight| **in_flight == lane).count()
    }

    fn has_room(&self, lane: Lane) -> bool {
        self.in_flight(lane) < self.workers.capacity(lane)
    }

    /// The lane for a job on data `key`, and whether the job can name the
    /// data rather than carry it: the lane that last got the data while it
    /// has room, or else the least loaded lane with room, which then becomes
    /// the data's lane. `None` if no lane has room. `full` asks for a lane
    /// that gets the data.
    pub(crate) fn place(&mut self, key: DataKey, full: bool) -> Option<(Lane, bool)> {
        if self.workers.caches_data()
            && !full
            && let Some(&lane) = self.hints.get(&key)
            && self.has_room(lane)
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

    /// Forgets which lane got data `key`, which a worker there lacked.
    pub(crate) fn forget(&mut self, key: DataKey) {
        self.hints.remove(&key);
    }

    /// The least loaded lane with room, for a job that carries all it
    /// reads; `None` if no lane has room.
    pub(crate) fn lane(&self) -> Option<Lane> {
        (0..self.workers.lanes()).map(Lane).filter(|lane| self.has_room(*lane)).min_by_key(|lane| self.in_flight(*lane))
    }

    /// Starts `run` on `lane`; its reply goes to `inbox`.
    pub(crate) fn submit(&mut self, lane: Lane, run: Run, inbox: &InboxSender) {
        self.stats.submitted += 1;
        self.in_flight.insert(run.id, lane);
        self.workers.submit(lane, run, inbox);
    }

    /// Logs what the jobs did, for `dump_debug_logs`.
    pub(crate) fn log_stats(&self) {
        let JobStats { submitted, received, needs_deps, needs_data, discover, build } = self.stats;
        if submitted == 0 {
            return;
        }
        let (sent, got) = self.workers.bytes();
        let share = |part: u64, whole: u64| if whole == 0 { 0.0 } else { part as f64 * 100.0 / whole as f64 };
        let layout = discover + build;
        let discover_share = if layout.is_zero() { 0.0 } else { discover.as_secs_f64() * 100.0 / layout.as_secs_f64() };
        let mut line = format!(
            "tile jobs: {submitted} submitted, {received} replied, {:.1}% NeedsDeps, {:.1}% NeedsData, \
             discovery {discover_share:.1}% of {:.1} ms layout",
            share(needs_deps, received),
            share(needs_data, received),
            layout.as_secs_f64() * 1000.0,
        );
        if sent > 0 || got > 0 {
            use std::fmt::Write as _;
            let _ = write!(line, ", {} B sent and {} B received per job", sent / submitted, got / received.max(1));
        }
        log::info!(target: log_target::TIMING, "{line}");
    }
}
