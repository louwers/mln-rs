//! What a scenario is, and [`Bench`], through which it measures.

use std::path::PathBuf;
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
/// `micro/expression/parse/liberty`, `macro/frame/street`,
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
/// which doesn't, or counts what the measured work started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetricKind {
    Count,
    Time,
    /// A count of the state that the scenario's setup left, such as the
    /// heap live when measuring began: what the counts start from, which
    /// `mln-bench compare` shows beside them and never marks.
    Context,
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
/// - `start_heap`: the heap bytes live when measuring began, what the
///   scenario's setup left (a [`MetricKind::Context`]);
/// - `peak_heap` and `absolute_peak_heap`: the most heap bytes live at
///   once, above `start_heap` and in all;
/// - `retained_heap` and `absolute_end_heap`: the heap bytes live at the
///   end, above `start_heap` and in all;
/// - `wall` and `cpu`: wall-clock time, and the CPU time of every thread;
/// - `gpu`: the time the GPU spent in the render passes of the frames
///   drawn, where the device times them ([`meter::gpu_time`]).
///
/// Each covers only what runs inside [`Bench::measure`], on every thread of
/// the process while it runs.
pub(crate) const STANDARD_METRICS: [&str; 11] = [
    "instructions",
    "allocations",
    "allocated",
    "start_heap",
    "peak_heap",
    "absolute_peak_heap",
    "retained_heap",
    "absolute_end_heap",
    "wall",
    "cpu",
    "gpu",
];

/// What one measured part counted, and how long it took.
#[derive(Clone, Copy)]
struct Part {
    instructions: Option<u64>,
    allocations: u64,
    allocated: u64,
    wall: Duration,
    /// `None` where no device timed the GPU's work.
    gpu: Option<Duration>,
}

/// What a scenario measures through: the measured parts of its run, its own
/// metrics, and what the environment adds that the runner can't see.
#[derive(Default)]
pub(crate) struct Bench {
    /// Live heap bytes when the first measured part began.
    start_heap: Option<i64>,
    measuring: bool,
    /// `None` once a part ran where the counters couldn't be read.
    instructions: Option<u64>,
    /// What each measured part counted, in order.
    parts: Vec<Part>,
    /// Whether each measured part is one unit of the scenario's work
    /// ([`Bench::per_part`]).
    per_part: bool,
    allocations: u64,
    allocated: u64,
    /// The most live heap bytes at once during any measured part, and those
    /// live at the end of the last.
    peak_heap: i64,
    end_heap: i64,
    wall: Duration,
    cpu: Duration,
    own: Vec<Metric>,
    /// What one unit of the scenario's work is, and how many the measured
    /// parts did ([`Bench::per`]).
    per: Option<(String, u64)>,
    pub(crate) gpu: Option<Gpu>,
    /// The hash of the fixture manifest the scenario read, which
    /// `support::fixtures::Fixtures::open` sets.
    pub(crate) fixtures: Option<String>,
    /// Where the scenario writes the last image it drew as a PNG, when
    /// `mln-bench run --images` asks for it.
    pub(crate) image: Option<PathBuf>,
}

impl Bench {
    pub(crate) fn new() -> Bench {
        Bench { instructions: Some(0), ..Bench::default() }
    }

    /// Runs `work` as a measured part of the scenario: the standard metrics
    /// add up over every part. Setup stays outside, and so does anything a
    /// scenario does between parts, such as reading back an image. The heap
    /// metrics count the live bytes when the first part began, the most live
    /// at once during any part and those live at the end of the last, so
    /// that what a scenario allocates in its setup shows apart from what its
    /// parts do.
    pub(crate) fn measure<R>(&mut self, work: impl FnOnce() -> R) -> R {
        assert!(!self.measuring, "measure() inside measure()");
        self.measuring = true;
        let heap = meter::heap();
        if self.start_heap.is_none() {
            self.start_heap = Some(heap.live);
        }
        meter::reset_peak();
        // The clocks last at the start and first at the end, so that they
        // leave out reading the counters. Reading the GPU's time takes what
        // it finished before the part, which the part leaves out.
        let gpu = meter::gpu_time();
        let instructions = meter::instructions();
        let cpu = meter::process_cpu();
        let wall = Instant::now();
        let result = work();
        let wall = wall.elapsed();
        let cpu = meter::process_cpu().saturating_sub(cpu);
        let instructions = instructions.zip(meter::instructions()).map(|(start, end)| end - start);
        // A device made during the part starts timing at zero.
        let gpu = meter::gpu_time().map(|end| end.saturating_sub(gpu.unwrap_or_default()));
        let end = meter::heap();

        self.instructions = self.instructions.zip(instructions).map(|(total, part)| total + part);
        let part = Part {
            instructions,
            allocations: end.allocations - heap.allocations,
            allocated: end.allocated - heap.allocated,
            wall,
            gpu,
        };
        self.parts.push(part);
        self.allocations += part.allocations;
        self.allocated += part.allocated;
        self.peak_heap = self.peak_heap.max(meter::peak());
        self.end_heap = end.live;
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

    /// Declares that the measured parts did `n` units of the scenario's
    /// work, such as frames or tiles, called `unit`: the run reports
    /// `instructions`, `allocations` and `allocated` per unit too, as
    /// `<metric>_per_<unit>`.
    pub(crate) fn per(&mut self, unit: &str, n: u64) {
        assert!(self.per.is_none(), "per() twice");
        assert!(n > 0, "{unit}: no units");
        self.per = Some((unit.to_owned(), n));
    }

    /// Declares that each measured part was one unit of the scenario's
    /// work, such as a frame, a tile or an edit, called `unit`: as
    /// [`per`](Self::per) does, and the run reports how the units' counts
    /// and times spread too, as `<metric>_per_<unit>_p50`, `_p90` and `_max`
    /// (nearest rank). The costliest units, such as a frame that crosses the
    /// antimeridian or the one where a tile arrives, show there however few
    /// they are among the rest.
    pub(crate) fn per_part(&mut self, unit: &str) {
        let parts = self.parts.len() as u64;
        assert!(parts >= 2, "{unit}: per_part needs two measured parts or more");
        self.per(unit, parts);
        self.per_part = true;
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
        let start_heap = self.start_heap.expect("the scenario measured nothing");
        let count =
            |name: &str, unit, value: f64| Metric { name: name.to_owned(), kind: MetricKind::Count, unit, value };
        let bytes = |name: &str, value: i64| count(name, Unit::Bytes, value as f64);
        let mut metrics = Vec::new();
        if let Some(instructions) = self.instructions {
            metrics.push(count("instructions", Unit::Count, instructions as f64));
        }
        metrics.push(count("allocations", Unit::Count, self.allocations as f64));
        metrics.push(count("allocated", Unit::Bytes, self.allocated as f64));
        metrics.push(Metric { kind: MetricKind::Context, ..bytes("start_heap", start_heap) });
        metrics.push(bytes("peak_heap", self.peak_heap - start_heap));
        metrics.push(bytes("absolute_peak_heap", self.peak_heap));
        metrics.push(bytes("retained_heap", self.end_heap - start_heap));
        metrics.push(bytes("absolute_end_heap", self.end_heap));
        let time = |name: &str, value| Metric { name: name.to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value };
        metrics.push(time("wall", ms(self.wall)));
        metrics.push(time("cpu", ms(self.cpu)));
        // A part without a time after a timed one ran once the GPU's time
        // became unknown.
        let mut timed = self.parts.iter().map(|part| part.gpu).skip_while(Option::is_none).peekable();
        if timed.peek().is_some()
            && let Some(gpu) = timed.sum::<Option<Duration>>()
        {
            metrics.push(time("gpu", ms(gpu)));
        }
        if let Some((unit, n)) = &self.per {
            let n = *n as f64;
            let per_unit: Vec<Metric> = metrics
                .iter()
                .filter(|metric| ["instructions", "allocations", "allocated"].contains(&metric.name.as_str()))
                .map(|metric| Metric {
                    name: format!("{}_per_{unit}", metric.name),
                    kind: MetricKind::Count,
                    unit: metric.unit,
                    value: metric.value / n,
                })
                .collect();
            metrics.extend(per_unit);
            if self.per_part {
                metrics.extend(spread_of_parts(&self.parts, unit));
            }
        }
        metrics.extend(self.own);
        metrics
    }
}

/// The 50th and 90th percentiles and the largest of each part's
/// instructions, allocations, bytes allocated, wall time, and GPU time
/// where every part has one.
fn spread_of_parts(parts: &[Part], unit: &str) -> Vec<Metric> {
    let mut metrics = Vec::new();
    let mut add = |name: &str, kind: MetricKind, metric_unit: Unit, mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        for (suffix, rank) in [("p50", 50), ("p90", 90), ("max", 100)] {
            let index = (values.len() * rank).div_ceil(100).max(1) - 1;
            let name = format!("{name}_per_{unit}_{suffix}");
            metrics.push(Metric { name, kind, unit: metric_unit, value: values[index] });
        }
    };
    let instructions: Option<Vec<f64>> = parts.iter().map(|part| part.instructions.map(|n| n as f64)).collect();
    if let Some(instructions) = instructions {
        add("instructions", MetricKind::Count, Unit::Count, instructions);
    }
    add("allocations", MetricKind::Count, Unit::Count, parts.iter().map(|part| part.allocations as f64).collect());
    add("allocated", MetricKind::Count, Unit::Bytes, parts.iter().map(|part| part.allocated as f64).collect());
    add("wall", MetricKind::Time, Unit::Ms, parts.iter().map(|part| ms(part.wall)).collect());
    let gpu: Option<Vec<f64>> = parts.iter().map(|part| part.gpu.map(ms)).collect();
    if let Some(gpu) = gpu {
        add("gpu", MetricKind::Time, Unit::Ms, gpu);
    }
    metrics
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
    fn the_heap_of_the_setup_counts_apart_from_the_parts() {
        let mut bench = Bench::new();
        let setup = std::hint::black_box(vec![0u8; 1 << 20]);
        let kept = bench.measure(|| std::hint::black_box(vec![1u8; 1 << 16]));
        let metrics = bench.finish();
        let get = |name: &str| metrics.iter().find(|metric| metric.name == name).unwrap();
        let value = |name: &str| get(name).value;
        assert_eq!(get("start_heap").kind, MetricKind::Context);
        assert!(value("start_heap") >= f64::from(1 << 20));
        for growth in ["peak_heap", "retained_heap"] {
            assert!((f64::from(1 << 16)..f64::from(1 << 20)).contains(&value(growth)), "{growth}: {}", value(growth));
        }
        assert!(value("absolute_peak_heap") >= f64::from((1 << 20) + (1 << 16)));
        assert!(value("absolute_end_heap") >= f64::from((1 << 20) + (1 << 16)));
        drop((setup, kept));
    }

    #[test]
    fn per_part_reports_how_the_parts_spread() {
        let mut bench = Bench::new();
        for size in 1..=10usize {
            // Part 10 allocates ten times, the rest once.
            let count = if size == 10 { 10 } else { 1 };
            bench.measure(|| (0..count).map(|_| std::hint::black_box(vec![0u8; 64])).collect::<Vec<_>>());
        }
        bench.per_part("frame");
        let metrics = bench.finish();
        let get = |name: &str| metrics.iter().find(|metric| metric.name == name).unwrap();
        assert!(get("allocations_per_frame_max").value >= 10.0);
        assert!(get("allocations_per_frame_p50").value < get("allocations_per_frame_max").value);
        assert_eq!(get("allocations_per_frame_max").kind, MetricKind::Count);
        assert_eq!(get("wall_per_frame_p90").kind, MetricKind::Time);
    }

    #[test]
    fn the_gpu_time_spreads_where_every_part_has_one() {
        let part = |gpu: Option<u64>| Part {
            instructions: None,
            allocations: 0,
            allocated: 0,
            wall: Duration::ZERO,
            gpu: gpu.map(Duration::from_millis),
        };
        let timed = spread_of_parts(&[part(Some(1)), part(Some(3))], "frame");
        let max = timed.iter().find(|metric| metric.name == "gpu_per_frame_max").unwrap();
        assert_eq!((max.kind, max.value), (MetricKind::Time, 3.0));
        let untimed = spread_of_parts(&[part(Some(1)), part(None)], "frame");
        assert!(untimed.iter().all(|metric| !metric.name.starts_with("gpu")));
    }

    #[test]
    fn the_gpu_time_adds_up_from_the_first_timed_part_while_it_is_known() {
        let gpu_of = |parts: &[Option<u64>]| {
            let mut bench = Bench::new();
            bench.measure(|| ());
            bench.parts = parts
                .iter()
                .map(|gpu| Part {
                    instructions: None,
                    allocations: 0,
                    allocated: 0,
                    wall: Duration::ZERO,
                    gpu: gpu.map(Duration::from_millis),
                })
                .collect();
            bench.finish().into_iter().find(|metric| metric.name == "gpu").map(|metric| metric.value)
        };
        assert_eq!(gpu_of(&[None, Some(1), Some(2)]), Some(3.0));
        assert_eq!(gpu_of(&[Some(1), None]), None);
        assert_eq!(gpu_of(&[None]), None);
    }

    #[test]
    fn a_process_without_a_timed_device_has_no_gpu_time() {
        let mut bench = Bench::new();
        bench.measure(|| std::hint::black_box(vec![1u8; 64]));
        assert!(bench.finish().iter().all(|metric| metric.name != "gpu"));
    }

    #[test]
    fn per_unit_metrics_divide_the_totals() {
        let mut bench = Bench::new();
        bench.measure(|| std::hint::black_box(vec![1u8; 4000]));
        bench.per("tile", 4);
        let metrics = bench.finish();
        let get = |name: &str| metrics.iter().find(|metric| metric.name == name).unwrap().value;
        assert_eq!(get("allocated_per_tile"), get("allocated") / 4.0);
        assert_eq!(get("allocations_per_tile"), get("allocations") / 4.0);
    }
}
