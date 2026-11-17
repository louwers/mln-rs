//! What a scenario is, and [`Bench`], through which it measures.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::environment::Gpu;
use crate::meter;

/// Whether a scenario runs one pipeline stage as a function over data, or a
/// map doing something a user waits for or feels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Micro,
    Macro,
}

/// One benchmark scenario, run in a process of its own.
///
/// `name` is a path of lowercase segments: the kind, then the pipeline stage
/// (micro) or the operation (macro), then the case or workload, and a size
/// last where the scenario runs at two:
/// `micro/expression/evaluate`, `macro/frame/inline`,
/// `micro/geojson/parse/large`.
pub(crate) struct Scenario {
    pub(crate) name: &'static str,
    pub(crate) kind: Kind,
    /// The one question its numbers answer, as `mln-bench list` and the
    /// README show it.
    pub(crate) question: &'static str,
    /// Sets the scenario up, measures it through the [`Bench`] and declares
    /// its own metrics there. It does the same work on every run and every
    /// machine. A failure panics, which fails the scenario's process.
    pub(crate) run: fn(&mut Bench),
}

/// Whether a metric counts work, which repeats across runs, or times it,
/// which doesn't.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MetricKind {
    Count,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Unit {
    /// A number of things: instructions, allocations, frames.
    Count,
    Bytes,
    /// Milliseconds, the unit of every time.
    Ms,
}

/// One number a scenario's run gave. Lower is better for every metric.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Metric {
    pub(crate) name: String,
    pub(crate) kind: MetricKind,
    pub(crate) unit: Unit,
    pub(crate) value: f64,
}

/// The metrics every scenario reports, in this order, before its own:
/// - `instructions`: retired by every thread of the process, where the
///   system counts them;
/// - `allocations` and `allocated`: heap allocations of every thread and
///   the bytes they asked for;
/// - `peak_heap` and `retained_heap`: the most heap bytes live at once, and
///   those live at the end, above what was live when measuring began;
/// - `wall` and `cpu`: wall-clock time, and the CPU time of every thread.
///
/// Each covers only what runs inside [`Bench::measure`].
pub(crate) const STANDARD_METRICS: [&str; 7] =
    ["instructions", "allocations", "allocated", "peak_heap", "retained_heap", "wall", "cpu"];

/// What a scenario measures through: the measured parts of its run, its own
/// metrics, and what the environment adds that the runner can't see.
#[derive(Default)]
pub(crate) struct Bench {
    /// Live heap bytes when the first measured part began.
    baseline: Option<i64>,
    measuring: bool,
    /// `None` once a part ran where the counters couldn't be read.
    instructions: Option<u64>,
    allocations: u64,
    allocated: u64,
    peak_heap: i64,
    retained_heap: i64,
    wall: Duration,
    cpu: Duration,
    own: Vec<Metric>,
    pub(crate) gpu: Option<Gpu>,
    /// The hash of the fixture manifest the scenario read, which
    /// `support::fixtures::Fixtures::open` sets.
    pub(crate) fixtures: Option<String>,
}

impl Bench {
    pub(crate) fn new() -> Bench {
        Bench { instructions: Some(0), ..Bench::default() }
    }

    /// Runs `work` as a measured part of the scenario: the standard metrics
    /// add up over every part. Setup stays outside, and so does anything a
    /// scenario does between parts, such as waiting for the GPU to finish a
    /// frame. The heap metrics count from the live bytes when the first part
    /// began, so what a scenario allocates in its setup counts in neither.
    pub(crate) fn measure<R>(&mut self, work: impl FnOnce() -> R) -> R {
        assert!(!self.measuring, "measure() inside measure()");
        self.measuring = true;
        let heap = meter::heap();
        if self.baseline.is_none() {
            self.baseline = Some(heap.live);
            meter::reset_peak();
        }
        // The clocks last at the start and first at the end, so that they
        // leave out reading the counters.
        let instructions = meter::instructions();
        let cpu = meter::process_cpu();
        let wall = Instant::now();
        let result = work();
        let wall = wall.elapsed();
        let cpu = meter::process_cpu().saturating_sub(cpu);
        let instructions = instructions.zip(meter::instructions()).map(|(start, end)| end - start);
        let end = meter::heap();
        let baseline = self.baseline.unwrap_or_default();

        self.instructions = self.instructions.zip(instructions).map(|(total, part)| total + part);
        self.allocations += end.allocations - heap.allocations;
        self.allocated += end.allocated - heap.allocated;
        self.peak_heap = meter::peak() - baseline;
        self.retained_heap = end.live - baseline;
        self.wall += wall;
        self.cpu += cpu;
        self.measuring = false;
        result
    }

    /// Declares a count of the scenario's own, such as frames drawn.
    pub(crate) fn count(&mut self, name: &str, value: u64) {
        self.add(name, MetricKind::Count, Unit::Count, value as f64);
    }

    /// Declares a count of bytes of the scenario's own, such as GPU buffers.
    pub(crate) fn bytes(&mut self, name: &str, value: u64) {
        self.add(name, MetricKind::Count, Unit::Bytes, value as f64);
    }

    /// Declares a time of the scenario's own.
    pub(crate) fn time(&mut self, name: &str, value: Duration) {
        self.add(name, MetricKind::Time, Unit::Ms, ms(value));
    }

    /// Declares the 50th, 90th and 99th percentiles of `samples`, such as
    /// frame times, as `<name>_p50`, `<name>_p90` and `<name>_p99`.
    pub(crate) fn percentiles(&mut self, name: &str, samples: &[Duration]) {
        assert!(!samples.is_empty(), "{name}: no samples");
        let mut sorted = samples.to_vec();
        sorted.sort();
        for percentile in [50, 90, 99] {
            // The nearest rank.
            let rank = (sorted.len() * percentile).div_ceil(100).max(1);
            self.time(&format!("{name}_p{percentile}"), sorted[rank - 1]);
        }
    }

    fn add(&mut self, name: &str, kind: MetricKind, unit: Unit, value: f64) {
        assert!(
            !STANDARD_METRICS.contains(&name) && self.own.iter().all(|metric| metric.name != name),
            "metric {name} declared twice"
        );
        self.own.push(Metric { name: name.to_owned(), kind, unit, value });
    }

    /// The standard metrics, then the scenario's own.
    pub(crate) fn finish(self) -> Vec<Metric> {
        assert!(self.baseline.is_some(), "the scenario measured nothing");
        let count =
            |name: &str, unit, value: f64| Metric { name: name.to_owned(), kind: MetricKind::Count, unit, value };
        let mut metrics = Vec::new();
        if let Some(instructions) = self.instructions {
            metrics.push(count("instructions", Unit::Count, instructions as f64));
        }
        metrics.push(count("allocations", Unit::Count, self.allocations as f64));
        metrics.push(count("allocated", Unit::Bytes, self.allocated as f64));
        metrics.push(count("peak_heap", Unit::Bytes, self.peak_heap as f64));
        metrics.push(count("retained_heap", Unit::Bytes, self.retained_heap as f64));
        let time = |name: &str, value| Metric { name: name.to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value };
        metrics.push(time("wall", ms(self.wall)));
        metrics.push(time("cpu", ms(self.cpu)));
        metrics.extend(self.own);
        metrics
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_only_inside_measure() {
        let mut bench = Bench::new();
        let setup = std::hint::black_box(vec![0u8; 1 << 20]);
        let kept = bench.measure(|| std::hint::black_box(vec![1u8; 4096]));
        bench.count("things", 3);
        drop(setup);
        let metrics = bench.finish();
        let get = |name: &str| metrics.iter().find(|metric| metric.name == name).unwrap().value;
        assert!(get("allocations") >= 1.0);
        assert!(get("allocated") >= 4096.0);
        assert_eq!(get("things"), 3.0);
        drop(kept);
    }

    #[test]
    fn percentiles_take_the_nearest_rank() {
        let mut bench = Bench::new();
        bench.measure(|| ());
        let samples: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        bench.percentiles("frame", &samples);
        let metrics = bench.finish();
        let get = |name: &str| metrics.iter().find(|metric| metric.name == name).unwrap().value;
        assert_eq!(get("frame_p50"), 50.0);
        assert_eq!(get("frame_p90"), 90.0);
        assert_eq!(get("frame_p99"), 99.0);
    }
}
