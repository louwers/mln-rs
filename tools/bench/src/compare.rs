//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref, both sides running this checkout's harness, or with
//! `--own-harness` each side its own.
//!
//! The ref is checked out in a Git worktree under `<target>/bench-compare`,
//! which the next comparison reuses, with this checkout's harness laid over
//! it: `tools/bench`, and the libraries' `bench` modules, the entry points
//! that micro scenarios call. Both sides build with this checkout's
//! toolchain and `profile.toml`, so they differ only in the libraries. When
//! the harness doesn't build against the ref, the comparison stops. Every
//! run reports its harness id (build.rs), and the comparison refuses runs
//! whose ids differ.
//!
//! With `--own-harness`, the ref builds as it is, its own harness included,
//! with this checkout's toolchain and `profile.toml`: for a ref whose
//! libraries this checkout's harness doesn't build against, such as one
//! from before a change to a `bench` module's API. The sides compare the
//! scenarios both harnesses have, by name, and the report warns that their
//! harnesses differ, so that a change can come from either harness.
//!
//! Counts: a scenario's instructions move with the bytes of its binary,
//! by up to a few percent in some scenarios, though the code it runs is the
//! same: any edit moves them, and so can building the same source in
//! another directory. So each side is measured as several builds of the
//! same code: its own, and [`PROBES`] probes whose bytes differ
//! ([`build`]). A side's count is the median over its builds, each build
//! counting once, and a count's noise is the larger interquartile range of
//! either side's builds. The ref's probe runs and its own build's runs are
//! kept under `<target>/bench-compare/samples`, by commit and by this
//! checkout's `profile.toml`, so a later comparison with the same ref and
//! profile builds and runs only this checkout's probes. The report shows every metric's change and its noise, and marks
//! a count that moved by over [`NOISE_FACTOR`] times its noise, however
//! little: one that repeats exactly is marked for any change. The reader
//! weighs what matters.
//! Times: one run per side has no noise to hold them against, unless
//! `--rounds` asks for interleaved rounds, which a rank test compares: the
//! wall-clock time, and the GPU's where the device times it. Rounds run
//! alone among the machine's benchmark runs ([`host::measuring`]), and not
//! while its load is over `--max-load`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::report::{Run, format_value, table};
use crate::runner::{run_in_process, warm_up};
use crate::scenario::{MetricKind, Scenario, Unit};
use crate::stats::{holm, interquartile_range, mann_whitney, median};
use crate::{host, scenarios};

/// The runs of the base's own build kept for each scenario, the latest.
const KEPT_RUNS: usize = 8;
/// How many times its noise a count must move by to be marked.
const NOISE_FACTOR: f64 = 2.0;
/// The probes of each side, besides its own build: a median over this
/// many builds is seldom one whose bytes moved a count far, which a single
/// build often is.
const PROBES: usize = 8;
/// A time is marked when its Holm-adjusted p-value is below this.
const ALPHA: f64 = 0.05;
/// The times that rounds test, each on its own: the wall-clock time, and
/// the GPU's, which on a GPU can change while the wall-clock time, mostly
/// the CPU's work, doesn't. A scenario's other times move with them.
const TESTED_TIMES: [&str; 2] = ["wall", "gpu"];

pub(crate) struct Options {
    pub(crate) base: String,
    pub(crate) filters: Vec<String>,
    /// Interleaved rounds per side, for times; none for one run per side.
    pub(crate) rounds: Option<usize>,
    /// The load average over which rounds don't run.
    pub(crate) max_load: f64,
    /// Whether the ref runs its own harness instead of this checkout's.
    pub(crate) own_harness: bool,
}

/// Which way a metric moved, when it moved beyond its noise. Lower is
/// better for every metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Moved {
    More,
    Less,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetricComparison {
    pub(crate) name: String,
    pub(crate) kind: MetricKind,
    pub(crate) unit: Unit,
    /// Each side's median over its builds ([`draws`]); none where the side
    /// lacks the metric.
    pub(crate) base: Option<f64>,
    pub(crate) candidate: Option<f64>,
    /// The candidate's change from the base, as a fraction of the base.
    pub(crate) change: Option<f64>,
    /// A count's noise, as a fraction of the base: the larger interquartile
    /// range of either side's builds.
    pub(crate) noise: Option<f64>,
    /// A tested time's p-value with rounds, Holm-adjusted over every
    /// scenario of the comparison.
    pub(crate) p: Option<f64>,
    /// Which way the metric moved, if beyond its noise.
    pub(crate) moved: Option<Moved>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScenarioComparison {
    pub(crate) scenario: String,
    pub(crate) environment: Environment,
    /// Why the scenario's changes aren't held against their noise, if they
    /// aren't.
    pub(crate) not_comparable: Option<String>,
    /// The runs the counts of the base used, and of this checkout, over
    /// all their builds.
    pub(crate) base_runs: usize,
    pub(crate) candidate_runs: usize,
    pub(crate) metrics: Vec<MetricComparison>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Comparison {
    pub(crate) base: String,
    pub(crate) base_commit: String,
    /// The harness ids of this checkout's runs and of the base's, which
    /// differ only with `--own-harness`.
    pub(crate) harness: String,
    pub(crate) base_harness: String,
    /// The scenarios of the filters that only the base's harness has, and
    /// that only this checkout's has, which aren't compared.
    pub(crate) only_base: Vec<String>,
    pub(crate) only_this: Vec<String>,
    pub(crate) rounds: Option<usize>,
    /// With rounds, where the system keeps a load average: the machine's
    /// before the rounds and after them. The one after counts the rounds'
    /// own work.
    pub(crate) load: Option<Load>,
    pub(crate) scenarios: Vec<ScenarioComparison>,
}

/// The machine's load average around a comparison's rounds.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) struct Load {
    pub(crate) before: f64,
    pub(crate) after: f64,
}

impl Comparison {
    /// How many metrics moved beyond their noise.
    pub(crate) fn moved(&self) -> usize {
        self.scenarios.iter().flat_map(|scenario| &scenario.metrics).filter(|metric| metric.moved.is_some()).count()
    }
}

/// Builds both sides, runs the scenarios `options.filters` selects and
/// compares them.
pub(crate) fn compare(options: &Options) -> Result<Comparison, String> {
    let selected = scenarios::select(&options.filters);
    if selected.is_empty() {
        return Err("no scenario matches the filters".to_owned());
    }
    if let Some(rounds) = options.rounds {
        let needed = min_rounds(selected.len());
        if rounds < needed {
            return Err(format!(
                "--rounds {rounds} can't tell a time's change from noise among {} scenarios: that takes at least {needed} \
                 rounds, or fewer scenarios (--filter)",
                selected.len()
            ));
        }
    }
    // The checkout the comparison runs in, found at run time: a path built
    // into the binary would differ between the sides' builds, and move the
    // data that their code addresses.
    let root = PathBuf::from(git(Path::new("."), &["rev-parse", "--show-toplevel"])?);
    if !root.join("tools/bench/profile.toml").exists() {
        return Err(format!("{} has no tools/bench to compare with", root.display()));
    }
    let commit = git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", options.base)])?;
    let own = options.own_harness;

    eprintln!("mln-bench: building this checkout");
    let candidate = build(&root, &root.join("Cargo.toml"), None, None, None)?;
    // <target>/benchmark/mln-bench: the comparison works in <target>.
    let target = candidate.parent().and_then(Path::parent).ok_or("the binary's target directory")?;
    let work = target.join("bench-compare");
    std::fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let _lock = lock(&work)?;
    // The probes of this checkout link to the same path, so it's copied out.
    let candidate = keep_binary(&candidate, &work.join("bin/this"))?;

    let whose = if own { "its own" } else { "this checkout's" };
    eprintln!("mln-bench: building {} ({}) with {whose} harness", options.base, short(&commit));
    let tree = work.join("tree");
    prepare_tree(&root, &tree, &commit, !own)?;
    let base_target = work.join("target");
    let base = build(&root, &tree.join("Cargo.toml"), Some(&base_target), None, None).map_err(|error| {
        if own {
            format!(
                "{error}\n\nThe harness of {} ({}) doesn't build with this checkout's toolchain and profile.toml.",
                options.base,
                short(&commit)
            )
        } else {
            format!(
                "{error}\n\nThis checkout's harness, with its libraries' bench modules, doesn't build against {} ({}). \
                 Compare with each side's own harness instead (--own-harness), or commit the library code a scenario \
                 or entry point needs with it, then compare later changes with that commit (tools/bench/README.md).",
                options.base,
                short(&commit)
            )
        }
    })?;
    // The ref's builds link to the same path, so each is copied out.
    let base = keep_binary(&base, &work.join("bin/base"))?;
    let (scenarios, only_base, only_this) = if own {
        match_scenarios(&selected, &scenario_names(&base, &options.filters)?)
    } else {
        (selected, vec![], vec![])
    };
    for (side, names) in [(options.base.as_str(), &only_base), ("this checkout", &only_this)] {
        if !names.is_empty() {
            eprintln!("mln-bench: only {side}'s harness has {}, which isn't compared", names.join(", "));
        }
    }
    if scenarios.is_empty() {
        return Err(format!("no scenario of the filters is in both harnesses, this checkout's and {}'s", options.base));
    }

    let _measuring = host::measuring(options.rounds.is_some())?;
    let load_before = options.rounds.and_then(|_| host::load_average());
    if let Some(load) = load_before
        && load > options.max_load
    {
        return Err(format!(
            "the machine's load average is {load:.1}, over --max-load {:.1}: times measured now would mislead. \
             Run the rounds when the machine is quiet, or compare counts without --rounds",
            options.max_load
        ));
    }

    let profile = read(&root.join("tools/bench/profile.toml"))?;
    let samples_path = work.join("samples").join(samples_name(&commit, &profile));
    let mut samples = read_samples(&samples_path);
    eprintln!("mln-bench: a first run of each scenario on each side, not counted");
    let base_warm_up = warm_up(&base, &scenarios)?;
    let base_harness = harness_of(&base_warm_up)?;
    let harness = harness_of(&warm_up(&candidate, &scenarios)?)?;
    if base_harness != harness {
        if !own {
            return Err(format!(
                "the sides ran different harness builds ({base_harness} and {harness}), so they can't be compared"
            ));
        }
        let changes = git(&root, &["diff", "--stat", &commit, "--", "tools/bench", "crates/*/src/bench.rs"])?;
        eprintln!(
            "mln-bench: warning: the sides run different harnesses, so a change can come from the harness as well as \
             the libraries:\n {changes}"
        );
    }
    let probe_key = |scenario: &Scenario| format!("{} (layout probe)", scenario.name);
    // The probes a scenario has of the base's harness, in the environment
    // that its comparison keeps.
    let probes_of = |samples: &Samples, scenario: &Scenario, warm_up: &Run| {
        samples.get(&probe_key(scenario)).map_or(0, |runs| {
            runs.iter().filter(|run| run.harness == base_harness && run.environment == warm_up.environment).count()
        })
    };
    let unprobed: Vec<&Scenario> = scenarios
        .iter()
        .zip(&base_warm_up)
        .filter(|(scenario, warm_up)| probes_of(&samples, scenario, warm_up) < PROBES)
        .map(|(scenario, _)| *scenario)
        .collect();
    if !unprobed.is_empty() {
        for scenario in &unprobed {
            if let Some(runs) = samples.get_mut(&probe_key(scenario)) {
                runs.retain(|run| run.harness != base_harness);
            }
        }
        // With the harness's `layout-probe` feature, where it has one, the
        // probes' code after the harness lies at other addresses too.
        let manifest = std::fs::read_to_string(tree.join("tools/bench/Cargo.toml")).unwrap_or_default();
        let features = manifest.contains("layout-probe").then_some("layout-probe");
        for index in 0..PROBES {
            eprintln!("mln-bench: probe {} of {PROBES} of {} ({})", index + 1, options.base, short(&commit));
            let probe = build(&root, &tree.join("Cargo.toml"), Some(&base_target), features, Some(index))?;
            let probe = keep_binary(&probe, &work.join("bin/probe"))?;
            warm_up(&probe, &unprobed)?;
            for scenario in &unprobed {
                let run = run_in_process(&probe, scenario.name, None)?;
                samples.entry(probe_key(scenario)).or_default().push(run);
            }
        }
        write_samples(&samples_path, &samples)?;
    }
    let changed_while_running =
        |scenario: &Scenario| format!("{}: a side's harness changed while it ran", scenario.name);
    // This checkout's probes, always with its harness's `layout-probe`
    // feature, which also gives them a path of their own in the target
    // directory, apart from the binary that runs this comparison.
    let mut candidate_probes: BTreeMap<&str, Vec<Run>> = BTreeMap::new();
    for index in 0..PROBES {
        eprintln!("mln-bench: probe {} of {PROBES} of this checkout", index + 1);
        let probe = build(&root, &root.join("Cargo.toml"), None, Some("layout-probe"), Some(index))?;
        let probe = keep_binary(&probe, &work.join("bin/probe"))?;
        warm_up(&probe, &scenarios)?;
        for scenario in &scenarios {
            let run = run_in_process(&probe, scenario.name, None)?;
            if run.harness != harness {
                return Err(changed_while_running(scenario));
            }
            candidate_probes.entry(scenario.name).or_default().push(run);
        }
    }
    let mut comparisons = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let (base_runs, candidate_runs) = run_sides(scenario, index, &base, &candidate, options.rounds)?;
        if base_runs.iter().any(|run| run.harness != base_harness)
            || candidate_runs.iter().any(|run| run.harness != harness)
        {
            return Err(changed_while_running(scenario));
        }
        let environment = &base_runs[0].environment;
        let kept = samples.entry(scenario.name.to_owned()).or_default();
        kept.retain(|run| run.harness == base_harness && &run.environment == environment);
        kept.extend(base_runs.iter().cloned());
        let excess = kept.len().saturating_sub(KEPT_RUNS);
        kept.drain(..excess);
        let base_own = kept.clone();
        let base_probes: Vec<Run> = samples
            .get(&probe_key(scenario))
            .into_iter()
            .flatten()
            .filter(|run| run.harness == base_harness && &run.environment == environment)
            .cloned()
            .collect();
        write_samples(&samples_path, &samples)?;
        let base = Side { runs: &base_runs, own: &base_own, probes: &base_probes };
        let probes = candidate_probes.get(scenario.name).map_or(&[][..], Vec::as_slice);
        let candidate = Side { runs: &candidate_runs, own: &candidate_runs, probes };
        comparisons.push(compare_scenario(scenario, &base, &candidate, options.rounds));
    }
    if options.rounds.is_some() {
        judge_times(&mut comparisons);
    }
    Ok(Comparison {
        base: options.base.clone(),
        base_commit: commit,
        harness,
        base_harness,
        only_base,
        only_this,
        rounds: options.rounds,
        load: load_before.zip(host::load_average()).map(|(before, after)| Load { before, after }),
        scenarios: comparisons,
    })
}

/// The harness id that every one of `runs`, a binary's, reports.
fn harness_of(runs: &[Run]) -> Result<String, String> {
    let harness = runs.first().map(|run| run.harness.clone()).ok_or("no runs")?;
    match runs.iter().find(|run| run.harness != harness) {
        Some(run) => Err(format!("one binary reported two harness ids, {harness} and {}", run.harness)),
        None => Ok(harness),
    }
}

/// The names of the scenarios that `filters` select in the harness `binary`.
fn scenario_names(binary: &Path, filters: &[String]) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Listed {
        name: String,
    }
    let mut command = Command::new(binary);
    command.args(["list", "--json"]);
    for filter in filters {
        command.args(["--filter", filter]);
    }
    let output = command.stderr(Stdio::inherit()).output().map_err(|error| format!("{}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!("{} list failed ({})", binary.display(), output.status));
    }
    let listed: Vec<Listed> =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("{} list: {error}", binary.display()))?;
    Ok(listed.into_iter().map(|listed| listed.name).collect())
}

/// The scenarios of `selected`, this checkout's, that the base's harness has
/// too, by name, in this checkout's order; then the names of those only the
/// base has, and of those only this checkout has.
fn match_scenarios<'a>(selected: &[&'a Scenario], base: &[String]) -> (Vec<&'a Scenario>, Vec<String>, Vec<String>) {
    let (both, only_this): (Vec<&Scenario>, Vec<&Scenario>) =
        selected.iter().partition(|scenario| base.iter().any(|name| name == scenario.name));
    let only_base =
        base.iter().filter(|name| selected.iter().all(|scenario| scenario.name != *name)).cloned().collect();
    (both, only_base, only_this.iter().map(|scenario| scenario.name.to_owned()).collect())
}

/// The fewest rounds with which a time can move beyond its noise when `tests`
/// scenarios each test it: Holm's adjustment multiplies the smallest
/// p-value by `tests`, and with n rounds per side the smallest p-value of a
/// two-sided Mann-Whitney U test is 2 / C(2n, n), when the sides don't
/// overlap at all.
fn min_rounds(tests: usize) -> usize {
    (2..=64)
        .find(|&n| {
            let orderings: f64 = (1..=n).map(|i| (n + i) as f64 / i as f64).product();
            2.0 / orderings * (tests as f64) < ALPHA
        })
        .unwrap_or(usize::MAX)
}

/// Gives each tested time of each scenario of `comparisons` its p-value
/// Holm-adjusted over all the scenarios, so that the chance of marking that
/// time anywhere when it didn't change stays under [`ALPHA`], and marks it
/// if it moved. Each tested time is a question of its own, and adjusting
/// over them all would raise the rounds a comparison needs.
fn judge_times(comparisons: &mut [ScenarioComparison]) {
    for name in TESTED_TIMES {
        let mut tested: Vec<&mut MetricComparison> = comparisons
            .iter_mut()
            .filter(|scenario| scenario.not_comparable.is_none())
            .flat_map(|scenario| scenario.metrics.iter_mut())
            .filter(|metric| metric.name == name && metric.kind == MetricKind::Time && metric.p.is_some())
            .collect();
        let p_values: Vec<f64> = tested.iter().map(|metric| metric.p.unwrap_or(1.0)).collect();
        for (metric, p) in tested.iter_mut().zip(holm(&p_values)) {
            metric.p = Some(p);
            if let (Some(b), Some(c), Some(change)) = (metric.base, metric.candidate, metric.change)
                && p < ALPHA
                && change != 0.0
            {
                metric.moved = Some(if c > b { Moved::More } else { Moved::Less });
            }
        }
    }
}

/// Runs `scenario` on both sides: once each, or `rounds` times each in
/// ABBA order, so that drift in the machine's load hits both alike. With
/// one run each, the side that goes first alternates between scenarios.
fn run_sides(
    scenario: &Scenario,
    index: usize,
    base: &Path,
    candidate: &Path,
    rounds: Option<usize>,
) -> Result<(Vec<Run>, Vec<Run>), String> {
    let mut base_runs = Vec::new();
    let mut candidate_runs = Vec::new();
    let rounds_count = rounds.unwrap_or(1);
    for round in 0..rounds_count {
        let base_first = match rounds {
            Some(_) => round % 4 == 0 || round % 4 == 3,
            None => index.is_multiple_of(2),
        };
        for is_base in [base_first, !base_first] {
            let side = if is_base { "base" } else { "this checkout" };
            eprintln!("mln-bench: {}: {side}, round {} of {rounds_count}", scenario.name, round + 1);
            if is_base {
                base_runs.push(run_in_process(base, scenario.name, None)?);
            } else {
                candidate_runs.push(run_in_process(candidate, scenario.name, None)?);
            }
        }
    }
    Ok((base_runs, candidate_runs))
}

/// One side's runs of one scenario.
struct Side<'a> {
    /// This comparison's runs of the side's own build, for times.
    runs: &'a [Run],
    /// The runs of its own build that its counts use: this comparison's,
    /// and for the base the kept ones too.
    own: &'a [Run],
    /// One run of each of its probes.
    probes: &'a [Run],
}

impl Side<'_> {
    fn all(&self) -> impl Iterator<Item = &Run> {
        self.own.iter().chain(self.probes)
    }
}

/// A count of `side` as each of its builds gave it: the median of its own
/// build's runs, then each probe's run. Each build counts once, however
/// many runs it has.
fn draws(side: &Side<'_>, name: &str) -> Vec<f64> {
    let values =
        |runs: &[Run]| -> Vec<f64> { runs.iter().filter_map(|run| run.metric(name)).map(|m| m.value).collect() };
    let own = values(side.own);
    let mut draws = values(side.probes);
    if !own.is_empty() {
        draws.insert(0, median(&own));
    }
    draws
}

/// The comparison of one scenario's runs on each side. With rounds, the
/// tested time gets its p-value unadjusted, and [`judge_times`] judges it.
fn compare_scenario(
    scenario: &Scenario,
    base_side: &Side<'_>,
    candidate_side: &Side<'_>,
    rounds: Option<usize>,
) -> ScenarioComparison {
    let (base_runs, candidate_runs) = (base_side.runs, candidate_side.runs);
    let environment = candidate_runs[0].environment.clone();
    let mut not_comparable = None;
    if let Some(run) = base_side.all().chain(candidate_side.all()).find(|run| run.environment != environment) {
        not_comparable = Some(format!(
            "the sides ran in different environments ({})",
            environment.differences(&run.environment).join("; ")
        ));
    } else if timed_gpu(base_side.own) != timed_gpu(candidate_side.own) {
        // A commit from before the libraries timed the GPU, against one
        // that does.
        not_comparable = Some("only one side timed the GPU, whose timestamps cost work of their own".to_owned());
    }
    let values = |runs: &[Run], name: &str| -> Vec<f64> {
        runs.iter().filter_map(|run| run.metric(name)).map(|metric| metric.value).collect()
    };
    // Every metric either side reported, in the candidate's order.
    let mut names: Vec<(&str, MetricKind, Unit)> = Vec::new();
    for run in candidate_runs.iter().chain(base_runs) {
        for metric in &run.metrics {
            if names.iter().all(|(name, ..)| *name != metric.name) {
                names.push((&metric.name, metric.kind, metric.unit));
            }
        }
    }
    let mut metrics: Vec<MetricComparison> = Vec::new();
    for (name, kind, unit) in names {
        let (base, candidate) = if kind == MetricKind::Time {
            (values(base_runs, name), values(candidate_runs, name))
        } else {
            (draws(base_side, name), draws(candidate_side, name))
        };
        let base_median = (!base.is_empty()).then(|| median(&base));
        let candidate_median = (!candidate.is_empty()).then(|| median(&candidate));
        let change = base_median.zip(candidate_median).map(|(b, c)| relative(c - b, b));
        let mut comparison = MetricComparison {
            name: name.to_owned(),
            kind,
            unit,
            base: base_median,
            candidate: candidate_median,
            change,
            noise: None,
            p: None,
            moved: None,
        };
        if let (Some(b), Some(c)) = (base_median, candidate_median)
            && kind != MetricKind::Time
        {
            let spread = interquartile_range(&base).max(interquartile_range(&candidate));
            comparison.noise = Some(relative(spread, b));
            if kind == MetricKind::Count && (c - b).abs() > NOISE_FACTOR * spread {
                comparison.moved = Some(if c > b { Moved::More } else { Moved::Less });
            }
        }
        if rounds.is_some() && kind == MetricKind::Time && TESTED_TIMES.contains(&name) {
            comparison.p = (!base.is_empty() && !candidate.is_empty()).then(|| mann_whitney(&base, &candidate));
        }
        metrics.push(comparison);
    }
    if not_comparable.is_some() {
        for metric in &mut metrics {
            metric.moved = None;
        }
    }
    ScenarioComparison {
        scenario: scenario.name.to_owned(),
        environment,
        not_comparable,
        base_runs: base_side.all().count(),
        candidate_runs: candidate_side.all().count(),
        metrics,
    }
}

/// Whether `runs` timed the GPU's render passes, which costs instructions
/// and allocations of its own.
fn timed_gpu(runs: &[Run]) -> bool {
    runs.iter().any(|run| run.metric("gpu").is_some())
}

/// `delta` as a fraction of `base`: 0 for no change, infinite for a change
/// from zero.
fn relative(delta: f64, base: f64) -> f64 {
    if delta == 0.0 {
        0.0
    } else if base == 0.0 {
        f64::INFINITY.copysign(delta)
    } else {
        delta / base.abs()
    }
}

/// The comparison, for reading.
pub(crate) fn text(comparison: &Comparison) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "mln-bench compare: this checkout against {} ({})",
        comparison.base,
        short(&comparison.base_commit)
    );
    if comparison.base_harness == comparison.harness {
        let _ = writeln!(out, "harness {} on both sides", comparison.harness);
    } else {
        let _ = writeln!(
            out,
            "harness {} on the base and {} on this checkout, each side's own: a change can come from either harness",
            comparison.base_harness, comparison.harness
        );
    }
    for (side, names) in [("the base", &comparison.only_base), ("this checkout", &comparison.only_this)] {
        if !names.is_empty() {
            let _ = writeln!(out, "only {side}'s harness has, not compared: {}", names.join(", "));
        }
    }
    let _ = writeln!(
        out,
        "counts: each side's median over its own build and {PROBES} probes whose bytes differ; noise: the larger \
         interquartile range of either side's builds; a change over {NOISE_FACTOR}x its noise is marked more or \
         less, except in start_heap, which the setup left"
    );
    match comparison.rounds {
        Some(rounds) => {
            let _ = writeln!(
                out,
                "times: {rounds} interleaved rounds; {} each marked when its Mann-Whitney p, Holm-adjusted over \
                 the scenarios, is under {ALPHA}",
                TESTED_TIMES.join(" and ")
            );
        }
        None => {
            let _ = writeln!(out, "times: one run per side, so no noise to hold them against (--rounds <n>)");
        }
    }
    if let Some(load) = comparison.load {
        let _ = writeln!(
            out,
            "load average: {:.1} before the rounds, {:.1} after, which counts the rounds' own work",
            load.before, load.after
        );
    }
    let mut environments: Vec<&Environment> = Vec::new();
    for scenario in &comparison.scenarios {
        if !environments.contains(&&scenario.environment) {
            environments.push(&scenario.environment);
        }
    }
    for environment in environments {
        let _ = writeln!(out, "environment: {environment}");
    }
    for scenario in &comparison.scenarios {
        let _ = writeln!(
            out,
            "\n{} (counts: {} base runs, {} of this checkout)",
            scenario.scenario, scenario.base_runs, scenario.candidate_runs
        );
        if let Some(reason) = &scenario.not_comparable {
            let _ = writeln!(out, "  not comparable: {reason}");
        }
        let mut rows = vec![["metric", "base", "this", "change", "noise", "moved"].map(str::to_owned).to_vec()];
        for metric in &scenario.metrics {
            let value =
                |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format_value(metric.unit, value));
            let noise = match (metric.noise, metric.p) {
                (Some(noise), _) => percent(noise, false),
                (None, Some(p)) => format!("p={p:.3}"),
                (None, None) => String::new(),
            };
            let moved = match metric.moved {
                Some(Moved::More) => "more",
                Some(Moved::Less) => "less",
                None => "",
            };
            rows.push(vec![
                metric.name.clone(),
                value(metric.base),
                value(metric.candidate),
                metric.change.map_or_else(String::new, |change| percent(change, true)),
                noise,
                moved.to_owned(),
            ]);
        }
        out.push_str(&table(&rows));
    }
    let moved = comparison.moved();
    let _ = writeln!(out, "\n{moved} change{} beyond noise", if moved == 1 { "" } else { "s" });
    out
}

fn percent(fraction: f64, signed: bool) -> String {
    if fraction.is_infinite() {
        return if fraction > 0.0 { "new".to_owned() } else { "gone".to_owned() };
    }
    let value = fraction * 100.0;
    let digits = if value.abs() < 1.0 { 2 } else { 1 };
    if signed { format!("{value:+.digits$}%") } else { format!("{value:.digits$}%") }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Builds the harness of the workspace `manifest` with the benchmark profile
/// of this checkout, with this checkout's toolchain, and the harness's
/// `features`, and returns the binary. With `probe`, it builds that probe of
/// [`PROBES`]: the harness crate gets another `-C metadata`, which renames
/// its symbols, so the binary's bytes differ while every scenario runs the
/// same code.
fn build(
    root: &Path,
    manifest: &Path,
    target_dir: Option<&Path>,
    features: Option<&str>,
    probe: Option<usize>,
) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    // The checkout's directory, whose rust-toolchain.toml rustup reads.
    command
        .current_dir(root)
        .arg(if probe.is_some() { "rustc" } else { "build" })
        .args(["--profile", "benchmark", "--config"])
        .arg(root.join("tools/bench/profile.toml"))
        .args(["-p", "mln-rs-bench", "--bin", "mln-bench", "--message-format", "json-render-diagnostics"])
        .arg("--manifest-path")
        .arg(manifest);
    if let Some(dir) = target_dir {
        // A base's build, the only one in a directory of its own. Its code
        // can go unused under this checkout's harness, such as a `bench`
        // hook that its libraries don't call, and its warnings are no one's
        // to fix, even in CI, which denies them.
        command.arg("--target-dir").arg(dir).env("CARGO_BUILD_WARNINGS", "allow");
    }
    if let Some(features) = features {
        command.args(["--features", features]);
    }
    if let Some(probe) = probe {
        // `cargo rustc` passes these to the harness crate alone.
        command.arg("--").arg("-C").arg(format!("metadata=mln-bench-probe-{probe}"));
    }
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!("building {} failed", manifest.display()));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact" && message["target"]["name"] == "mln-bench")
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .ok_or_else(|| format!("building {} gave no mln-bench binary", manifest.display()))
}

/// Copies the binary `built` to `kept`, unless it holds the same already,
/// and returns `kept`.
fn keep_binary(built: &Path, kept: &Path) -> Result<PathBuf, String> {
    if let Some(dir) = kept.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    write_if_changed(kept, &read(built)?)?;
    let permissions = std::fs::metadata(built).map_err(|error| format!("{}: {error}", built.display()))?.permissions();
    std::fs::set_permissions(kept, permissions).map_err(|error| format!("{}: {error}", kept.display()))?;
    Ok(kept.to_owned())
}

/// Checks `commit` out at `tree`, a worktree of the checkout's repository,
/// with `tools/bench` a member of its workspace, and with `overlay` lays
/// this checkout's harness over it: `tools/bench`, and each library's
/// `bench` module, in place of the commit's. A file is written only where
/// its contents differ, so that Cargo builds again only what changed since
/// the last comparison.
fn prepare_tree(root: &Path, tree: &Path, commit: &str, overlay: bool) -> Result<(), String> {
    if tree.join(".git").exists() {
        // With the overlay, the tree keeps the harness of the last
        // comparison when it has the commit already. Without, it goes back
        // to the commit's: checking out writes only the files that differ.
        if !overlay || git(tree, &["rev-parse", "HEAD"]).ok().as_deref() != Some(commit) {
            git(tree, &["checkout", "-q", "--force", "--detach", commit])?;
            git(tree, &["clean", "-q", "-d", "--force"])?;
        }
    } else {
        if tree.exists() {
            std::fs::remove_dir_all(tree).map_err(|error| format!("{}: {error}", tree.display()))?;
        }
        let path = tree.to_str().ok_or("a path that isn't UTF-8")?;
        git(root, &["worktree", "add", "-q", "--force", "--detach", path, commit])?;
    }
    if overlay {
        sync_dir(&root.join("tools/bench"), &tree.join("tools/bench"))?;
        let modules = bench_modules(root)?;
        for module in &modules {
            let path = tree.join(module);
            if !path.parent().is_some_and(Path::is_dir) {
                return Err(format!(
                    "{} has no {}, so this checkout's harness, which calls its {}, can't build against it: compare \
                     with each side's own harness instead (--own-harness)",
                    short(commit),
                    module.parent().unwrap_or(module).display(),
                    module.display()
                ));
            }
            write_if_changed(&path, &read(&root.join(module))?)?;
        }
        // A module that only the commit has goes back to the commit's.
        for module in bench_modules(tree)? {
            if !modules.contains(&module) {
                let path = module.to_str().ok_or("a path that isn't UTF-8")?.replace('\\', "/");
                if git(tree, &["checkout", "-q", commit, "--", &path]).is_err() {
                    let path = tree.join(&module);
                    std::fs::remove_file(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                }
            }
        }
    }
    let manifest_path = tree.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    write_if_changed(&manifest_path, with_member(&manifest)?.as_bytes())
}

/// The libraries' `bench` modules in the checkout at `root`, which belong to
/// the harness: `crates/<library>/src/bench.rs`, relative to `root`.
fn bench_modules(root: &Path) -> Result<Vec<PathBuf>, String> {
    let crates = root.join("crates");
    let mut modules = Vec::new();
    for entry in std::fs::read_dir(&crates).map_err(|error| format!("{}: {error}", crates.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let module = Path::new("crates").join(entry.file_name()).join("src/bench.rs");
        if root.join(&module).is_file() {
            modules.push(module);
        }
    }
    modules.sort();
    Ok(modules)
}

/// The workspace manifest `manifest` with `tools/bench` among its members.
fn with_member(manifest: &str) -> Result<String, String> {
    if manifest.contains("\"tools/bench\"") {
        return Ok(manifest.to_owned());
    }
    let members = "members = [";
    let at = manifest.find(members).ok_or("the base's Cargo.toml lists no workspace members")? + members.len();
    Ok(format!("{}\n  \"tools/bench\",{}", &manifest[..at], &manifest[at..]))
}

/// Makes the directory `to` hold what `from` holds, leaving out build
/// output, and writes only the files whose contents differ.
fn sync_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if entry.file_name() == "target" {
            continue;
        }
        if path.is_dir() {
            sync_dir(&path, &to.join(entry.file_name()))?;
        } else {
            write_if_changed(&to.join(entry.file_name()), &read(&path)?)?;
        }
        names.push(entry.file_name());
    }
    for entry in std::fs::read_dir(to).map_err(|error| format!("{}: {error}", to.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name() == "target" || names.contains(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Writes `contents` to `path` unless it holds them already.
fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    std::fs::write(path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

/// Holds the comparison's lock in `work` until it drops, so that two
/// comparisons from one checkout take turns with its worktree.
fn lock(work: &Path) -> Result<File, String> {
    let path = work.join("lock");
    let file = File::create(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.try_lock().is_err() {
        eprintln!("mln-bench: waiting for another comparison in this checkout");
        file.lock().map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(file)
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

type Samples = BTreeMap<String, Vec<Run>>;

/// The file of the base's kept runs: by commit, and by `profile`, the
/// `profile.toml` that both sides build with, which the base's harness id
/// covers only when the harness is this checkout's.
fn samples_name(commit: &str, profile: &[u8]) -> String {
    // FNV-1a, which every build of the tool computes the same way.
    let hash = profile
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3));
    format!("{commit}-{hash:016x}.json")
}

/// The base's kept runs, or none when there are none or they don't read.
fn read_samples(path: &Path) -> Samples {
    std::fs::read_to_string(path).ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

fn write_samples(path: &Path, samples: &Samples) -> Result<(), String> {
    let dir = path.parent().ok_or("the samples' directory")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let json = serde_json::to_string(samples).map_err(|error| error.to_string())?;
    std::fs::write(path, json).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{Kind, Metric};

    fn run(instructions: f64, wall: f64) -> Run {
        run_with(instructions, 1000.0, wall)
    }

    fn run_with(instructions: f64, allocations: f64, wall: f64) -> Run {
        let count = |name: &str, kind, value| Metric { name: name.to_owned(), kind, unit: Unit::Count, value };
        run_of(vec![
            count("instructions", MetricKind::Count, instructions),
            count("allocations", MetricKind::Count, allocations),
            count("allocations_per_tile", MetricKind::Count, allocations / 10.0),
            Metric { name: "wall".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
            Metric { name: "cpu".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
            Metric { name: "gpu".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: 1.0 },
        ])
    }

    fn run_of(metrics: Vec<Metric>) -> Run {
        Run {
            scenario: "micro/test/case".to_owned(),
            kind: Kind::Micro,
            harness: "h".to_owned(),
            environment: Environment::machine(),
            metrics,
        }
    }

    /// A run whose heap held `start` bytes when measuring began and at most
    /// `growth` more during it.
    fn heap_run(start: f64, growth: f64) -> Run {
        let bytes = |name: &str, kind, value| Metric { name: name.to_owned(), kind, unit: Unit::Bytes, value };
        run_of(vec![
            bytes("start_heap", MetricKind::Context, start),
            bytes("peak_heap", MetricKind::Count, growth),
            bytes("absolute_peak_heap", MetricKind::Count, start + growth),
        ])
    }

    const SCENARIO: Scenario = Scenario { name: "micro/test/case", kind: Kind::Micro, question: "?", run: |_| {} };

    fn moved(comparison: &ScenarioComparison, name: &str) -> Option<Moved> {
        comparison.metrics.iter().find(|metric| metric.name == name).unwrap().moved
    }

    /// A side of one build, its own, with `runs`; the first is this
    /// comparison's.
    fn own(runs: &[Run]) -> Side<'_> {
        Side { runs: &runs[..1], own: runs, probes: &[] }
    }

    /// A side whose own build ran `runs`, with one run of each probe.
    fn built<'a>(runs: &'a [Run], probes: &'a [Run]) -> Side<'a> {
        Side { runs: &runs[..1], own: runs, probes }
    }

    #[test]
    fn a_count_moves_beyond_twice_its_noise() {
        // Builds at 998, 1000, 1002, 1004 and 1006: an interquartile range,
        // the noise, of 4 about a median of 1002.
        let runs = [run(1002.0, 1.0)];
        let probes = [998.0, 1000.0, 1004.0, 1006.0].map(|instructions| run(instructions, 1.0));
        let base = built(&runs, &probes);
        let comparison = compare_scenario(&SCENARIO, &base, &own(&[run(1011.0, 5.0)]), None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::More));
        // Times have no noise to hold against without rounds.
        assert_eq!(moved(&comparison, "wall"), None);
        let comparison = compare_scenario(&SCENARIO, &base, &own(&[run(1009.0, 1.0)]), None);
        assert_eq!(moved(&comparison, "instructions"), None);
        let comparison = compare_scenario(&SCENARIO, &base, &own(&[run(993.0, 1.0)]), None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::Less));
        // The noise is the larger of the sides': builds of this checkout at
        // 1000 to 1040 hide a change of 20.
        let candidate_probes = [1000.0, 1010.0, 1030.0, 1040.0].map(|instructions| run(instructions, 1.0));
        let candidate_runs = [run(1020.0, 1.0)];
        let candidate = built(&candidate_runs, &candidate_probes);
        let comparison = compare_scenario(&SCENARIO, &base, &candidate, None);
        assert_eq!(moved(&comparison, "instructions"), None);
    }

    #[test]
    fn a_side_counts_each_build_once_and_takes_their_median() {
        let probes = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0), run(1001.0, 1.0)];
        // The base's own build kept four runs at 1030, which count once.
        let kept = [run(1030.0, 1.0), run(1030.0, 1.0), run(1030.0, 1.0), run(1030.0, 1.0)];
        let base = built(&kept, &probes);
        // This checkout's own build moved its count by 3%, as a build's
        // bytes can, but its probes didn't.
        let candidate_runs = [run(1030.0, 1.0)];
        let candidate = built(&candidate_runs, &probes);
        let comparison = compare_scenario(&SCENARIO, &base, &candidate, None);
        let instructions = comparison.metrics.iter().find(|metric| metric.name == "instructions").unwrap();
        assert_eq!((instructions.base, instructions.candidate), (Some(1000.0), Some(1000.0)));
        assert_eq!(instructions.moved, None);
        assert_eq!(comparison.base_runs, 8);
        // Every build of this checkout moved it.
        let moved_probes = probes.clone().map(|mut probe| {
            probe.metrics[0].value += 30.0;
            probe
        });
        let comparison = compare_scenario(&SCENARIO, &base, &built(&candidate_runs, &moved_probes), None);
        assert_eq!(moved(&comparison, "instructions"), Some(Moved::More));
    }

    #[test]
    fn a_count_that_repeats_moves_beyond_its_noise_with_any_change() {
        let runs = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        // One allocation more, and its mean per tile with it.
        let comparison = compare_scenario(&SCENARIO, &own(&runs), &own(&[run_with(1000.0, 1001.0, 1.0)]), None);
        assert_eq!(moved(&comparison, "allocations"), Some(Moved::More));
        assert_eq!(moved(&comparison, "instructions"), None);
        assert_eq!(moved(&comparison, "allocations_per_tile"), Some(Moved::More));
    }

    #[test]
    fn a_time_needs_rounds_and_a_significant_change_to_move() {
        let base: Vec<Run> = (0..6).map(|i| run(1000.0, 10.0 + f64::from(i) * 0.01)).collect();
        let slower: Vec<Run> = (0..6).map(|i| run(1000.0, 11.0 + f64::from(i) * 0.01)).collect();
        let same: Vec<Run> = (0..6).map(|i| run(1000.0, 10.005 + f64::from(i) * 0.01)).collect();
        let all = |runs| Side { runs, own: runs, probes: &[] };
        let mut comparisons = vec![
            compare_scenario(&SCENARIO, &all(&base), &all(&slower), Some(6)),
            compare_scenario(&SCENARIO, &all(&base), &all(&same), Some(6)),
        ];
        judge_times(&mut comparisons);
        assert_eq!(moved(&comparisons[0], "wall"), Some(Moved::More));
        // Only wall and gpu are tested.
        assert_eq!(moved(&comparisons[0], "cpu"), None);
        assert_eq!(moved(&comparisons[1], "wall"), None);
    }

    #[test]
    fn the_gpu_time_is_tested_on_its_own() {
        let with_gpu = |runs: &mut Vec<Run>, gpu: f64| {
            for (i, run) in runs.iter_mut().enumerate() {
                run.metrics.iter_mut().find(|metric| metric.name == "gpu").unwrap().value = gpu + i as f64 * 0.01;
            }
        };
        let mut base: Vec<Run> = (0..4).map(|_| run(1000.0, 10.0)).collect();
        let mut slower: Vec<Run> = (0..4).map(|_| run(1000.0, 10.0)).collect();
        with_gpu(&mut base, 1.0);
        with_gpu(&mut slower, 2.0);
        let all = |runs| Side { runs, own: runs, probes: &[] };
        let mut comparisons = vec![compare_scenario(&SCENARIO, &all(&base), &all(&slower), Some(4))];
        judge_times(&mut comparisons);
        // Four rounds suffice for one scenario: the wall time's test doesn't
        // raise the gpu time's p-value.
        assert_eq!(moved(&comparisons[0], "gpu"), Some(Moved::More));
        assert_eq!(moved(&comparisons[0], "wall"), None);
    }

    #[test]
    fn rounds_grow_with_the_scenarios_tested() {
        // 2 / C(8, 4) = 0.029 for one scenario; 2 / C(14, 7) x 50 = 0.029.
        assert_eq!(min_rounds(1), 4);
        assert_eq!(min_rounds(2), 5);
        assert_eq!(min_rounds(50), 7);
    }

    #[test]
    fn different_environments_are_not_compared() {
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut other = run(2000.0, 1.0);
        other.environment.cpu = "another".to_owned();
        let comparison = compare_scenario(&SCENARIO, &own(&base), &own(&[other]), None);
        assert!(comparison.not_comparable.is_some());
        assert_eq!(moved(&comparison, "instructions"), None);
    }

    #[test]
    fn a_side_that_didnt_time_the_gpu_is_not_compared() {
        let base_runs = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut untimed = run(1050.0, 1.0);
        untimed.metrics.retain(|metric| metric.name != "gpu");
        let comparison = compare_scenario(&SCENARIO, &own(&base_runs), &own(&[untimed]), None);
        assert!(comparison.not_comparable.is_some());
        assert_eq!(moved(&comparison, "instructions"), None);
    }

    #[test]
    fn the_heap_moves_by_its_absolute_peak_and_its_growth_never_its_start() {
        let runs = [heap_run(100.0, 10.0), heap_run(100.0, 10.0), heap_run(100.0, 10.0)];
        let verdict = |candidate: Run| {
            let comparison = compare_scenario(&SCENARIO, &own(&runs), &own(&[candidate]), None);
            let start = comparison.metrics.iter().find(|metric| metric.name == "start_heap").unwrap();
            assert!(start.noise.is_some() && start.moved.is_none());
            (moved(&comparison, "absolute_peak_heap"), moved(&comparison, "peak_heap"))
        };
        // A lower start, with the same growth: less in all, the same growth.
        assert_eq!(verdict(heap_run(50.0, 10.0)), (Some(Moved::Less), None));
        // A lower start doesn't hide more growth.
        assert_eq!(verdict(heap_run(50.0, 30.0)), (Some(Moved::Less), Some(Moved::More)));
        // A higher start, with the same growth: more in all.
        assert_eq!(verdict(heap_run(120.0, 10.0)), (Some(Moved::More), None));
    }

    #[test]
    fn matches_the_sides_scenarios_by_name() {
        const fn scenario(name: &'static str) -> Scenario {
            Scenario { name, kind: Kind::Micro, question: "?", run: |_| {} }
        }
        let (a, b, c) = (scenario("micro/a"), scenario("micro/b"), scenario("micro/c"));
        let base = ["micro/c".to_owned(), "micro/old".to_owned(), "micro/a".to_owned()];
        let (both, only_base, only_this) = match_scenarios(&[&a, &b, &c], &base);
        assert_eq!(both.iter().map(|scenario| scenario.name).collect::<Vec<_>>(), ["micro/a", "micro/c"]);
        assert_eq!(only_base, ["micro/old"]);
        assert_eq!(only_this, ["micro/b"]);
    }

    /// Runs `git args` in `dir`, as a committer of its own.
    fn git_in(dir: &Path, args: &[&str]) -> String {
        let identity =
            ["-c", "user.name=mln-bench", "-c", "user.email=mln-bench@localhost", "-c", "commit.gpgsign=false"];
        let output = Command::new("git").arg("-C").arg(dir).args(identity).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn contents(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn prepares_the_refs_tree_with_this_checkouts_harness_or_its_own() {
        let (repository, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, tree) = (repository.path(), work.path().join("tree"));
        git_in(root, &["init", "-q"]);
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\n  \"crates/*\",\n]\n");
        write(&root.join("crates/lib/src/bench.rs"), "the ref's");
        write(&root.join("tools/bench/src/main.rs"), "the ref's");
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "the ref"]);
        let commit = git_in(root, &["rev-parse", "HEAD"]);
        write(&root.join("crates/lib/src/bench.rs"), "this checkout's");
        write(&root.join("tools/bench/src/main.rs"), "this checkout's");
        write(&root.join("tools/bench/src/new.rs"), "this checkout's");

        let (module, harness, new) = (
            tree.join("crates/lib/src/bench.rs"),
            tree.join("tools/bench/src/main.rs"),
            tree.join("tools/bench/src/new.rs"),
        );
        prepare_tree(root, &tree, &commit, false).unwrap();
        assert_eq!((contents(&module), contents(&harness)), ("the ref's".to_owned(), "the ref's".to_owned()));
        assert!(!new.exists());
        assert!(contents(&tree.join("Cargo.toml")).contains("\"tools/bench\""));

        prepare_tree(root, &tree, &commit, true).unwrap();
        assert_eq!(
            (contents(&module), contents(&harness)),
            ("this checkout's".to_owned(), "this checkout's".to_owned())
        );
        assert!(new.exists());

        // The tree of the same commit, used again without the overlay, goes
        // back to the ref's harness.
        prepare_tree(root, &tree, &commit, false).unwrap();
        assert_eq!((contents(&module), contents(&harness)), ("the ref's".to_owned(), "the ref's".to_owned()));
        assert!(!new.exists());
    }

    #[test]
    fn the_overlay_of_a_module_whose_crate_the_ref_lacks_points_to_the_refs_own_harness() {
        let (repository, work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, tree) = (repository.path(), work.path().join("tree"));
        git_in(root, &["init", "-q"]);
        write(&root.join("Cargo.toml"), "[workspace]\nmembers = [\n  \"crates/*\",\n]\n");
        write(&root.join("crates/lib/src/bench.rs"), "the ref's");
        write(&root.join("tools/bench/src/main.rs"), "the ref's");
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "the ref"]);
        let commit = git_in(root, &["rev-parse", "HEAD"]);
        write(&root.join("crates/new/src/bench.rs"), "this checkout's");

        let error = prepare_tree(root, &tree, &commit, true).unwrap_err();
        assert!(error.contains("--own-harness"), "{error}");
        prepare_tree(root, &tree, &commit, false).unwrap();
    }

    #[test]
    fn keeps_the_bases_runs_by_commit_and_profile() {
        let name = samples_name("abc", b"codegen-units = 1");
        assert_eq!(name, samples_name("abc", b"codegen-units = 1"));
        assert_ne!(name, samples_name("abc", b"codegen-units = 16"));
        assert_ne!(name, samples_name("abd", b"codegen-units = 1"));
    }

    #[test]
    fn adds_the_harness_to_the_workspace_once() {
        let manifest = "[workspace]\nmembers = [\n  \"crates/*\",\n]\n";
        let with = with_member(manifest).unwrap();
        assert!(with.contains("\"tools/bench\""));
        assert_eq!(with_member(&with).unwrap(), with);
    }
}
