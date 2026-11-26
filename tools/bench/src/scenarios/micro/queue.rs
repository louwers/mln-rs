//! The queue a thread that owns some state runs its work from
//! (`mln_engine::exec::queue`): the runtime's thread takes an application's commands
//! from one, and the online file source's thread keeps the deadlines of its
//! requests in flight on one. What a task costs on its way through, apart
//! from its own work.

use std::hint::black_box;
use std::time::Duration;

use mln_engine::exec::queue::{Queue, TimedTask};

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/queue/commands/small",
        kind: Kind::Micro,
        question: "What does a command cost through a thread's queue and a turn of it, sent one per turn?",
        run: commands_small,
    },
    Scenario {
        name: "micro/queue/commands/large",
        kind: Kind::Micro,
        question: "What does a command cost through a thread's queue and a turn of it, sent 256 per turn?",
        run: commands_large,
    },
    Scenario {
        name: "micro/queue/timers/small",
        kind: Kind::Micro,
        question: "What do the deadlines of 64 requests in flight cost a thread's queue?",
        run: timers_small,
    },
    Scenario {
        name: "micro/queue/timers/large",
        kind: Kind::Micro,
        question: "What do the deadlines of 4,096 requests in flight cost a thread's queue?",
        run: timers_large,
    },
];

/// The commands the scenarios send, in turns of as many as each sends at
/// once.
const COMMANDS: usize = 1 << 16;

/// What a command carries here: a camera, as a command that moves the map
/// does: its center's latitude and longitude, zoom, bearing and pitch.
type Camera = [f64; 5];

/// Commands that each carry a camera, sent one at a time, each followed by
/// a turn of the queue, as an application that moves the map once a frame
/// does to the runtime's thread. The queue runs on the scenario's thread, so
/// waking another thread isn't part of it.
fn commands_small(bench: &mut Bench) {
    commands(bench, 1);
}

/// [`commands_small`] with 256 commands sent before each turn, as an
/// application that sets up a map in one go does.
fn commands_large(bench: &mut Bench) {
    commands(bench, 256);
}

fn commands(bench: &mut Bench, per_turn: usize) {
    let queue = Queue::new();
    let mut random = Random::new(0xc0de);
    let cameras: Vec<Camera> = (0..COMMANDS)
        .map(|_| {
            [
                random.range(-85.0, 85.0),
                random.range(-180.0, 180.0),
                random.range(0.0, 22.0),
                random.range(0.0, 360.0),
                random.range(0.0, 60.0),
            ]
        })
        .collect();
    let mut state = Camera::default();
    // One measured part for all the turns: measuring a part has a cost of
    // its own, which would be most of a turn's.
    bench.measure(|| {
        for turn in cameras.chunks(per_turn) {
            for &camera in turn {
                queue.post(move |state: &mut Camera| *state = black_box(camera));
            }
            assert!(queue.run_once(&mut state), "the turn runs the commands");
        }
    });
    assert_eq!(state, cameras[COMMANDS - 1], "the last command ran last");
    bench.per("command", COMMANDS as u64);
}

/// The deadlines that the scenarios keep, in rounds of as many as are in
/// flight at once.
const TIMERS: usize = 1 << 17;

/// The deadlines of 64 requests in flight at once, as the online file
/// source keeps them while a map loads a view: in each round, a timed task
/// posted for each request for when its response expires, from a minute to
/// a day ahead; half of them cancelled and posted again for another time, as
/// when a revalidation answers; every eighth cancelled and posted again to
/// run at once, as when a request is retried; one turn of the queue, which
/// runs those; then every deadline left cancelled with its request.
fn timers_small(bench: &mut Bench) {
    timers(bench, 64);
}

/// [`timers_small`] with 4,096 requests in flight at once, as an offline
/// download has.
fn timers_large(bench: &mut Bench) {
    timers(bench, 4096);
}

fn timers(bench: &mut Bench, in_flight: usize) {
    let queue = Queue::new();
    let mut fired = 0usize;
    let mut random = Random::new(0x7173);
    let rounds = TIMERS / in_flight;
    // Two expiries for each deadline of each round: its first, and a later
    // one.
    let expiries: Vec<Duration> =
        (0..2 * TIMERS).map(|_| Duration::from_secs(random.range(60.0, 86_400.0) as u64)).collect();
    let fire = |fired: &mut usize| *fired += 1;
    // One measured part for all the rounds: measuring a part has a cost of
    // its own, which would be most of a round's.
    bench.measure(|| {
        for expiries in expiries.chunks(2 * in_flight) {
            let mut timers: Vec<TimedTask> =
                expiries[..in_flight].iter().map(|&expiry| queue.post_after(expiry, fire)).collect();
            for (timer, &expiry) in timers.iter_mut().zip(&expiries[in_flight..]).step_by(2) {
                queue.cancel(*timer);
                *timer = queue.post_after(expiry, fire);
            }
            for timer in timers.iter_mut().step_by(8) {
                queue.cancel(*timer);
                *timer = queue.post_after(Duration::ZERO, fire);
            }
            queue.run_once(&mut fired);
            for timer in timers {
                queue.cancel(timer);
            }
        }
    });
    assert_eq!(fired, rounds * in_flight.div_ceil(8), "the retried deadlines pass, and only they");
    assert_eq!(queue.next_deadline(), None, "no deadline is left");
    bench.per("timer", TIMERS as u64);
}
