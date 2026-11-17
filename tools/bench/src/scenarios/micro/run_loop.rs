//! Timers on a run loop, as a file source keeps one for each request it
//! has in flight: started when a response says when it expires, started
//! again when it is revalidated or retried, and dropped with its request.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use mln_actor::{RunLoop, Timer};

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/run-loop/timers/small",
        kind: Kind::Micro,
        question: "What do the timers of 64 requests in flight cost a run loop?",
        run: timers_small,
    },
    Scenario {
        name: "micro/run-loop/timers/large",
        kind: Kind::Micro,
        question: "What do the timers of 4,096 requests in flight cost a run loop?",
        run: timers_large,
    },
];

/// The timers that the scenarios run, in rounds of as many as are in
/// flight at once.
const TIMERS: usize = 1 << 17;

/// The timers of 64 requests in flight at once, as a map that loads a view
/// has: in each round, a timer made for each request and started for when
/// its response expires, from a minute to a day ahead; half of them started
/// again for another time, as when a revalidation answers; every eighth
/// started again to fire at once, as when a request is retried; one turn of
/// the loop, which fires those; then every timer dropped with its request.
fn timers_small(bench: &mut Bench) {
    timers(bench, 64);
}

/// [`timers_small`] with 4,096 requests in flight at once, as an offline
/// download has.
fn timers_large(bench: &mut Bench) {
    timers(bench, 4096);
}

fn timers(bench: &mut Bench, in_flight: usize) {
    let run_loop = RunLoop::new();
    let fired = Rc::new(Cell::new(0));
    let mut random = Random::new(0x7173);
    let rounds = TIMERS / in_flight;
    // Two expiries for each timer of each round: its first, and a later one.
    let expiries: Vec<Duration> =
        (0..2 * TIMERS).map(|_| Duration::from_secs(random.range(60.0, 86_400.0) as u64)).collect();
    // One measured part for all the rounds: measuring a part has a cost of
    // its own, which would be most of a round's.
    bench.measure(|| {
        for expiries in expiries.chunks(2 * in_flight) {
            let timers: Vec<Timer> = (0..in_flight).map(|_| Timer::new(&run_loop)).collect();
            for (timer, expiry) in timers.iter().zip(expiries) {
                let fired = Rc::clone(&fired);
                timer.start(*expiry, Duration::ZERO, move || fired.set(fired.get() + 1));
            }
            for (timer, expiry) in timers.iter().zip(&expiries[in_flight..]).step_by(2) {
                let fired = Rc::clone(&fired);
                timer.start(*expiry, Duration::ZERO, move || fired.set(fired.get() + 1));
            }
            for timer in timers.iter().step_by(8) {
                let fired = Rc::clone(&fired);
                timer.start(Duration::ZERO, Duration::ZERO, move || fired.set(fired.get() + 1));
            }
            run_loop.run_once();
            drop(timers);
        }
    });
    assert_eq!(fired.get(), rounds * in_flight.div_ceil(8), "the retried timers fire, and only they");
    bench.per("timer", TIMERS as u64);
}
