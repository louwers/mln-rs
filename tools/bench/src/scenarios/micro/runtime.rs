//! The runtime's command queue: what a command an application sends to a
//! map costs on its way to the runtime's thread, posted, woken for and run,
//! apart from the command's own work.
//!
//! The queue is the runtime's, reached through its `bench` entry points
//! (`mln_runtime::bench`).

use std::hint::black_box;

use mln_runtime::bench::CommandQueue;

use crate::scenario::{Bench, Kind, Scenario};
use crate::support::random::Random;

pub(crate) const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "micro/runtime/commands/small",
        kind: Kind::Micro,
        question: "What does a command cost through the runtime's queue and a turn of its loop, sent one per turn?",
        run: commands_small,
    },
    Scenario {
        name: "micro/runtime/commands/large",
        kind: Kind::Micro,
        question: "What does a command cost through the runtime's queue and a turn of its loop, sent 256 per turn?",
        run: commands_large,
    },
];

/// The commands the scenarios send, in turns of as many as each sends at
/// once.
const COMMANDS: usize = 1 << 16;

/// What a command carries here: a camera, as a command that moves the map
/// does: its center's latitude and longitude, zoom, bearing and pitch.
type Camera = [f64; 5];

/// Commands that each carry a camera, sent one at a time, each followed by
/// a turn of the runtime's thread, as an application that moves the map
/// once a frame does: each posts its command to the queue and wakes the
/// thread's loop, and the turn runs the loop, then the command. The queue
/// and the loop run on the scenario's thread, so waking another thread
/// isn't part of it.
fn commands_small(bench: &mut Bench) {
    commands(bench, 1);
}

/// [`commands_small`] with 256 commands sent before each turn, as an
/// application that sets up a map in one go does.
fn commands_large(bench: &mut Bench) {
    commands(bench, 256);
}

fn commands(bench: &mut Bench, per_turn: usize) {
    let queue = CommandQueue::new();
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
            assert!(queue.run(&mut state), "the turn runs the commands");
        }
    });
    assert_eq!(state, cameras[COMMANDS - 1], "the last command ran last");
    bench.per("command", COMMANDS as u64);
}
