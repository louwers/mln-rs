//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref, both sides running this checkout's harness.
//!
//! The ref is checked out in a Git worktree under `<target>/bench-compare`,
//! which the next comparison reuses, with this checkout's `tools/bench` laid
//! over it. Both sides build with this checkout's toolchain and
//! `profile.toml`, so they differ only in the libraries. When the harness
//! doesn't build against the ref, the comparison stops: it never runs the
//! ref's own harness, whose scenarios may differ from this one's. Every run
//! reports its harness id (build.rs), and the comparison refuses runs whose
//! ids differ.
//!
//! Counts: each side runs once per scenario, and the ref's runs are kept
//! under `<target>/bench-compare/samples`, by commit, to measure how much a
//! count moves from run to run (its spread) without running the ref again
//! next time; the ref runs again only until it has [`SPREAD_RUNS`] of them.
//! Times: context only, unless `--rounds` asks for interleaved rounds, which
//! a rank test then judges.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::environment::Environment;
use crate::report::{Run, format_value, table};
use crate::runner::run_in_process;
use crate::scenario::{MetricKind, Scenario, Unit};
use crate::scenarios;
use crate::stats::{holm, mann_whitney, median, range};

/// The base's runs a count's spread needs at least.
const SPREAD_RUNS: usize = 3;
/// The base's runs kept for each scenario, the latest.
const KEPT_RUNS: usize = 8;
/// How many times the larger spread of the two sides a count must move by.
const SPREAD_FACTOR: f64 = 2.0;
/// A time's verdict needs its Holm-adjusted p-value below this.
const ALPHA: f64 = 0.05;
/// The fewest rounds with which the rank test can reach [`ALPHA`].
const MIN_ROUNDS: usize = 4;

pub(crate) struct Options {
    pub(crate) base: String,
    pub(crate) filters: Vec<String>,
    /// Interleaved rounds per side, for times; none for one run per side.
    pub(crate) rounds: Option<usize>,
    /// The smallest change of a count that gets a verdict, in percent.
    pub(crate) threshold: f64,
    /// The smallest change of a time's median that gets a verdict, in
    /// percent.
    pub(crate) time_threshold: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Better,
    Worse,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MetricComparison {
    pub(crate) name: String,
    pub(crate) kind: MetricKind,
    pub(crate) unit: Unit,
    /// The median of each side's runs; none where the side lacks the metric.
    pub(crate) base: Option<f64>,
    pub(crate) candidate: Option<f64>,
    /// The candidate's change from the base, as a fraction of the base.
    pub(crate) change: Option<f64>,
    /// A count's spread: the larger range of either side's runs, as a
    /// fraction of the base.
    pub(crate) spread: Option<f64>,
    /// A time's Holm-adjusted p-value, with rounds.
    pub(crate) p: Option<f64>,
    pub(crate) verdict: Option<Verdict>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ScenarioComparison {
    pub(crate) scenario: String,
    pub(crate) environment: Environment,
    /// Why the scenario gets no verdicts, if it doesn't.
    pub(crate) no_verdicts: Option<String>,
    /// The base's runs its counts used, and the candidate's.
    pub(crate) base_runs: usize,
    pub(crate) candidate_runs: usize,
    pub(crate) metrics: Vec<MetricComparison>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Comparison {
    pub(crate) base: String,
    pub(crate) base_commit: String,
    pub(crate) harness: String,
    pub(crate) rounds: Option<usize>,
    pub(crate) threshold: f64,
    pub(crate) time_threshold: f64,
    pub(crate) scenarios: Vec<ScenarioComparison>,
}

impl Comparison {
    pub(crate) fn verdicts(&self) -> usize {
        self.scenarios.iter().flat_map(|scenario| &scenario.metrics).filter(|metric| metric.verdict.is_some()).count()
    }
}

/// Builds both sides, runs the scenarios `options.filters` selects and
/// compares them.
pub(crate) fn compare(options: &Options) -> Result<Comparison, String> {
    if options.rounds.is_some_and(|rounds| rounds < MIN_ROUNDS) {
        return Err(format!("--rounds needs at least {MIN_ROUNDS}, the fewest with which a time can get a verdict"));
    }
    let scenarios = scenarios::select(&options.filters);
    if scenarios.is_empty() {
        return Err("no scenario matches the filters".to_owned());
    }
    // The checkout the comparison runs in, found at run time: a path built
    // into the binary would differ between the sides' builds, and move the
    // data that their code addresses.
    let root = PathBuf::from(git(Path::new("."), &["rev-parse", "--show-toplevel"])?);
    if !root.join("tools/bench/profile.toml").exists() {
        return Err(format!("{} has no tools/bench to compare with", root.display()));
    }
    let commit = git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", options.base)])?;

    eprintln!("mln-bench: building this checkout");
    let candidate = build(&root, &root.join("Cargo.toml"), None)?;
    // <target>/benchmark/mln-bench: the comparison works in <target>.
    let target = candidate.parent().and_then(Path::parent).ok_or("the binary's target directory")?;
    let work = target.join("bench-compare");
    std::fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let _lock = lock(&work)?;

    eprintln!("mln-bench: building {} ({}) with this checkout's harness", options.base, short(&commit));
    let tree = work.join("tree");
    prepare_tree(&root, &tree, &commit)?;
    let base = build(&root, &tree.join("Cargo.toml"), Some(&work.join("target"))).map_err(|error| {
        format!(
            "{error}\n\nThis checkout's harness doesn't build against {} ({}), so there are no verdicts. \
             A scenario that needs library code the base lacks can't be compared with it: commit that code \
             with the scenario, then compare later changes with that commit (tools/bench/README.md).",
            options.base,
            short(&commit)
        )
    })?;

    let samples_path = work.join("samples").join(format!("{commit}.json"));
    let mut samples = read_samples(&samples_path);
    let mut comparisons = Vec::new();
    for (index, scenario) in scenarios.iter().enumerate() {
        let (base_runs, candidate_runs) = run_sides(scenario, index, &base, &candidate, options.rounds)?;
        let harness = &candidate_runs[0].harness;
        if let Some(run) = base_runs.iter().chain(&candidate_runs).find(|run| &run.harness != harness) {
            return Err(format!(
                "{}: the sides ran different harness builds ({} and {harness}), so there are no verdicts",
                scenario.name, run.harness
            ));
        }
        let kept = samples.entry(scenario.name.to_owned()).or_default();
        let environment = &base_runs[0].environment;
        kept.retain(|run| &run.harness == harness && &run.environment == environment);
        let mut base_counts = kept.clone();
        base_counts.extend(base_runs.iter().cloned());
        while base_counts.len() < SPREAD_RUNS {
            eprintln!("mln-bench: {}: another base run, for the spread", scenario.name);
            base_counts.push(run_in_process(&base, scenario.name)?);
        }
        let fresh = base_counts.len() - kept.len();
        kept.extend(base_counts[base_counts.len() - fresh..].iter().cloned());
        let excess = kept.len().saturating_sub(KEPT_RUNS);
        kept.drain(..excess);
        write_samples(&samples_path, &samples)?;
        comparisons.push(compare_scenario(scenario, &base_runs, &base_counts, &candidate_runs, options));
    }
    Ok(Comparison {
        base: options.base.clone(),
        base_commit: commit,
        harness: env!("MLN_BENCH_HARNESS").to_owned(),
        rounds: options.rounds,
        threshold: options.threshold,
        time_threshold: options.time_threshold,
        scenarios: comparisons,
    })
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
                base_runs.push(run_in_process(base, scenario.name)?);
            } else {
                candidate_runs.push(run_in_process(candidate, scenario.name)?);
            }
        }
    }
    Ok((base_runs, candidate_runs))
}

/// The comparison of one scenario's runs: `base_counts` are the base's runs
/// for its counts, these and kept ones; `base_runs` and `candidate_runs`
/// this comparison's, for times.
fn compare_scenario(
    scenario: &Scenario,
    base_runs: &[Run],
    base_counts: &[Run],
    candidate_runs: &[Run],
    options: &Options,
) -> ScenarioComparison {
    let environment = candidate_runs[0].environment.clone();
    let mut no_verdicts = None;
    if let Some(run) = base_runs.iter().chain(candidate_runs).find(|run| run.environment != environment) {
        no_verdicts = Some(format!(
            "the sides ran in different environments ({})",
            environment.differences(&run.environment).join("; ")
        ));
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
    let mut metrics = Vec::new();
    for (name, kind, unit) in names {
        let base = if kind == MetricKind::Count { values(base_counts, name) } else { values(base_runs, name) };
        let candidate = values(candidate_runs, name);
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
            spread: None,
            p: None,
            verdict: None,
        };
        if let (Some(b), Some(c)) = (base_median, candidate_median)
            && kind == MetricKind::Count
        {
            let spread = range(&base).max(range(&candidate));
            comparison.spread = Some(relative(spread, b));
            let noise = (options.threshold / 100.0 * b.abs()).max(SPREAD_FACTOR * spread);
            if (c - b).abs() > noise {
                comparison.verdict = Some(if c > b { Verdict::Worse } else { Verdict::Better });
            }
        }
        metrics.push((comparison, base, candidate));
    }
    // Times: a rank test over the rounds, adjusted over the scenario's times.
    if options.rounds.is_some() {
        let times: Vec<usize> = (0..metrics.len())
            .filter(|&i| metrics[i].0.kind == MetricKind::Time && !metrics[i].1.is_empty() && !metrics[i].2.is_empty())
            .collect();
        let p_values: Vec<f64> = times.iter().map(|&i| mann_whitney(&metrics[i].1, &metrics[i].2)).collect();
        for (&i, p) in times.iter().zip(holm(&p_values)) {
            let comparison = &mut metrics[i].0;
            comparison.p = Some(p);
            if let (Some(b), Some(c), Some(change)) = (comparison.base, comparison.candidate, comparison.change)
                && p < ALPHA
                && change.abs() > options.time_threshold / 100.0
            {
                comparison.verdict = Some(if c > b { Verdict::Worse } else { Verdict::Better });
            }
        }
    }
    let mut metrics: Vec<MetricComparison> = metrics.into_iter().map(|(comparison, ..)| comparison).collect();
    if no_verdicts.is_some() {
        for metric in &mut metrics {
            metric.verdict = None;
        }
    }
    ScenarioComparison {
        scenario: scenario.name.to_owned(),
        environment,
        no_verdicts,
        base_runs: base_counts.len(),
        candidate_runs: candidate_runs.len(),
        metrics,
    }
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
    let _ = writeln!(out, "harness {} on both sides", comparison.harness);
    let _ = writeln!(
        out,
        "counts: a verdict needs a change over {}% and over {SPREAD_FACTOR}x the larger spread of either side's runs",
        comparison.threshold
    );
    match comparison.rounds {
        Some(rounds) => {
            let _ = writeln!(
                out,
                "times: {rounds} interleaved rounds; a verdict needs Holm-adjusted Mann-Whitney p < {ALPHA} and a change of the median over {}%",
                comparison.time_threshold
            );
        }
        None => {
            let _ = writeln!(out, "times: context only, from one run per side (--rounds <n> tests them)");
        }
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
        if let Some(reason) = &scenario.no_verdicts {
            let _ = writeln!(out, "  no verdicts: {reason}");
        }
        let mut rows = vec![["metric", "base", "this", "change", "spread", "verdict"].map(str::to_owned).to_vec()];
        for metric in &scenario.metrics {
            let value =
                |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format_value(metric.unit, value));
            let spread = match (metric.spread, metric.p) {
                (Some(spread), _) => percent(spread, false),
                (None, Some(p)) => format!("p={p:.3}"),
                (None, None) => String::new(),
            };
            let verdict = match metric.verdict {
                Some(Verdict::Better) => "better",
                Some(Verdict::Worse) => "WORSE",
                None if metric.kind == MetricKind::Time && comparison.rounds.is_none() => "(context)",
                None => "",
            };
            rows.push(vec![
                metric.name.clone(),
                value(metric.base),
                value(metric.candidate),
                metric.change.map_or_else(String::new, |change| percent(change, true)),
                spread,
                verdict.to_owned(),
            ]);
        }
        out.push_str(&table(&rows));
    }
    let verdicts = comparison.verdicts();
    let _ = writeln!(out, "\n{verdicts} verdict{}", if verdicts == 1 { "" } else { "s" });
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
/// of this checkout, with this checkout's toolchain, and returns the
/// binary.
fn build(root: &Path, manifest: &Path, target_dir: Option<&Path>) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    // The checkout's directory, whose rust-toolchain.toml rustup reads.
    command
        .current_dir(root)
        .args(["build", "--profile", "benchmark", "--config"])
        .arg(root.join("tools/bench/profile.toml"))
        .args(["-p", "mln-rs-bench", "--bin", "mln-bench", "--message-format", "json-render-diagnostics"])
        .arg("--manifest-path")
        .arg(manifest);
    if let Some(dir) = target_dir {
        command.arg("--target-dir").arg(dir);
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

/// Checks `commit` out at `tree`, a worktree of the checkout's repository,
/// and lays this checkout's harness over it as a member of its workspace.
fn prepare_tree(root: &Path, tree: &Path, commit: &str) -> Result<(), String> {
    if tree.join(".git").exists() {
        git(tree, &["checkout", "-q", "--force", "--detach", commit])?;
        git(tree, &["clean", "-q", "-d", "--force"])?;
    } else {
        if tree.exists() {
            std::fs::remove_dir_all(tree).map_err(|error| format!("{}: {error}", tree.display()))?;
        }
        let path = tree.to_str().ok_or("a path that isn't UTF-8")?;
        git(root, &["worktree", "add", "-q", "--force", "--detach", path, commit])?;
    }
    let harness = tree.join("tools/bench");
    if harness.exists() {
        std::fs::remove_dir_all(&harness).map_err(|error| format!("{}: {error}", harness.display()))?;
    }
    copy_dir(&root.join("tools/bench"), &harness)?;
    let manifest_path = tree.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    let manifest = with_member(&manifest)?;
    std::fs::write(&manifest_path, manifest).map_err(|error| error.to_string())
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

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if entry.file_name() == "target" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()))?;
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
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
        Run {
            scenario: "micro/test/case".to_owned(),
            kind: Kind::Micro,
            harness: "h".to_owned(),
            environment: Environment::machine(),
            metrics: vec![
                Metric {
                    name: "instructions".to_owned(),
                    kind: MetricKind::Count,
                    unit: Unit::Count,
                    value: instructions,
                },
                Metric { name: "wall".to_owned(), kind: MetricKind::Time, unit: Unit::Ms, value: wall },
            ],
        }
    }

    fn options(rounds: Option<usize>) -> Options {
        Options { base: "main".to_owned(), filters: Vec::new(), rounds, threshold: 1.0, time_threshold: 2.0 }
    }

    const SCENARIO: Scenario = Scenario { name: "micro/test/case", kind: Kind::Micro, question: "?", run: |_| {} };

    fn verdict(comparison: &ScenarioComparison, name: &str) -> Option<Verdict> {
        comparison.metrics.iter().find(|metric| metric.name == name).unwrap().verdict
    }

    #[test]
    fn a_count_needs_the_threshold_and_twice_the_spread() {
        let base = [run(1000.0, 1.0), run(1001.0, 1.0), run(1002.0, 1.0)];
        // +2%: over 1% and over twice the spread of 2.
        let comparison = compare_scenario(&SCENARIO, &base[..1], &base, &[run(1021.0, 5.0)], &options(None));
        assert_eq!(verdict(&comparison, "instructions"), Some(Verdict::Worse));
        // Times are context without rounds.
        assert_eq!(verdict(&comparison, "wall"), None);
        // +0.5%: under the threshold.
        let comparison = compare_scenario(&SCENARIO, &base[..1], &base, &[run(1006.0, 1.0)], &options(None));
        assert_eq!(verdict(&comparison, "instructions"), None);
        // A spread of 30 hides a change of 50.
        let noisy = [run(1000.0, 1.0), run(1015.0, 1.0), run(1030.0, 1.0)];
        let comparison = compare_scenario(&SCENARIO, &noisy[..1], &noisy, &[run(1065.0, 1.0)], &options(None));
        assert_eq!(verdict(&comparison, "instructions"), None);
        let comparison = compare_scenario(&SCENARIO, &noisy[..1], &noisy, &[run(900.0, 1.0)], &options(None));
        assert_eq!(verdict(&comparison, "instructions"), Some(Verdict::Better));
    }

    #[test]
    fn a_time_needs_rounds_and_a_significant_change() {
        let base: Vec<Run> = (0..6).map(|i| run(1000.0, 10.0 + f64::from(i) * 0.01)).collect();
        let slower: Vec<Run> = (0..6).map(|i| run(1000.0, 11.0 + f64::from(i) * 0.01)).collect();
        let comparison = compare_scenario(&SCENARIO, &base, &base, &slower, &options(Some(6)));
        assert_eq!(verdict(&comparison, "wall"), Some(Verdict::Worse));
        let same: Vec<Run> = (0..6).map(|i| run(1000.0, 10.005 + f64::from(i) * 0.01)).collect();
        let comparison = compare_scenario(&SCENARIO, &base, &base, &same, &options(Some(6)));
        assert_eq!(verdict(&comparison, "wall"), None);
    }

    #[test]
    fn different_environments_get_no_verdicts() {
        let base = [run(1000.0, 1.0), run(1000.0, 1.0), run(1000.0, 1.0)];
        let mut other = run(2000.0, 1.0);
        other.environment.cpu = "another".to_owned();
        let comparison = compare_scenario(&SCENARIO, &base[..1], &base, &[other], &options(None));
        assert!(comparison.no_verdicts.is_some());
        assert_eq!(verdict(&comparison, "instructions"), None);
    }

    #[test]
    fn adds_the_harness_to_the_workspace_once() {
        let manifest = "[workspace]\nmembers = [\n  \"crates/*\",\n]\n";
        let with = with_member(manifest).unwrap();
        assert!(with.contains("\"tools/bench\""));
        assert_eq!(with_member(&with).unwrap(), with);
    }
}
