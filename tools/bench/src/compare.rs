//! `mln-bench compare <ref>`: this checkout, uncommitted changes included,
//! against a Git ref (README.md).
//!
//! The ref is checked out in a worktree under `target/bench-compare`, which
//! the next comparison reuses, with this checkout's `tools/bench` laid over
//! it, so both sides run the same cases. When the harness doesn't build
//! there, because a case uses API the ref lacks, the ref runs its own
//! harness instead: the cases only this checkout has are reported as such,
//! and a case whose code differs between the two harnesses gets no verdict
//! (`changed_cases`). Each round runs one `mln-bench run` process per side,
//! in ABBA order so that drift hits both alike; a
//! process's value of a metric is one sample, since iterations in one
//! process share its caches and heap. A lead metric gets a verdict when a
//! two-sided Mann-Whitney U test, Holm-corrected over the case's lead
//! metrics, gives p < 0.05 and the medians differ by at least the
//! threshold. The first round keeps each side's last frames, and the report
//! says how many pixels differ, so a side that got faster by drawing less
//! shows up.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::stats::{holm, mann_whitney, median};
use crate::{Metrics, Report, cases};

pub struct Options {
    pub base: String,
    /// Processes per side; even, for ABBA.
    pub rounds: usize,
    /// The smallest change of a median, in percent, that gets a verdict.
    pub threshold: f64,
    /// Arguments both sides' `mln-bench run` get: filters and scale.
    pub run_args: Vec<String>,
    pub fixtures: PathBuf,
    /// Also report the metrics that get no verdict.
    pub verbose: bool,
}

const ALPHA: f64 = 0.05;

/// Runs the comparison and returns its Markdown report, and the case and
/// metric of each worse verdict.
pub fn compare(options: &Options) -> Result<(String, Vec<(String, String)>), String> {
    if options.rounds < 2 || !options.rounds.is_multiple_of(2) {
        return Err("--rounds must be even, and at least 2".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().map_err(|e| e.to_string())?;
    let work = root.join("target/bench-compare");
    let commit = git(&root, &["rev-parse", "--verify", &format!("{}^{{commit}}", options.base)])?;
    let (base_bin, own_harness) = build_base(&root, &work, &commit)?;
    let changed = if own_harness { changed_cases(&root, &work.join("tree"))? } else { BTreeSet::new() };
    let candidate_bin = std::env::current_exe().map_err(|e| e.to_string())?;

    let frames = work.join("frames");
    let _ = std::fs::remove_dir_all(&frames);
    let load_before = load_average();
    let mut runs: BTreeMap<Side, Vec<Report>> = BTreeMap::new();
    for round in 0..options.rounds {
        let order = if round % 4 == 0 || round % 4 == 3 {
            [Side::Base, Side::Candidate]
        } else {
            [Side::Candidate, Side::Base]
        };
        for side in order {
            eprintln!("mln-bench: round {} of {}: {}", round + 1, options.rounds, side.name());
            let bin = if side == Side::Base { &base_bin } else { &candidate_bin };
            let frames = (round == 0).then(|| frames.join(side.name()));
            runs.entry(side).or_default().push(run(bin, options, frames.as_deref())?);
        }
    }
    let load_after = load_average();

    let mut out = String::new();
    let base_env = &runs[&Side::Base][0].environment;
    let candidate_env = &runs[&Side::Candidate][0].environment;
    let short = &commit[..commit.len().min(12)];
    let _ = writeln!(out, "# mln-bench compare\n");
    let _ =
        writeln!(out, "- base: `{}` ({short}); candidate: this checkout, uncommitted changes included", options.base);
    let _ = writeln!(out, "- GPU: {}", candidate_env.gpu);
    let _ = writeln!(
        out,
        "- {} processes per side, ABBA; a verdict needs Holm-adjusted Mann-Whitney p < {ALPHA} and a change of at least {}%",
        options.rounds, options.threshold
    );
    if own_harness {
        let ran = &runs[&Side::Candidate][0].cases;
        let cases: Vec<String> =
            ran.iter().filter(|case| changed.contains(&case.name)).map(|case| format!("`{}`", case.name)).collect();
        let cases = if cases.is_empty() { "none".to_string() } else { cases.join(", ") };
        let _ = writeln!(
            out,
            "- the base doesn't build this checkout's harness, so it ran its own; the cases whose code differs between the two get no verdicts: {cases}"
        );
    }
    if let (Some(before), Some(after)) = (load_before, load_after) {
        let _ = writeln!(out, "- load average: {before} before, {after} after");
    }
    let comparable = base_env == candidate_env;
    if !comparable {
        let _ = writeln!(
            out,
            "\n**No verdicts: the sides ran on different setups.**\n- base: {base_env:?}\n- candidate: {candidate_env:?}"
        );
    }
    if candidate_env.software_gpu {
        let _ = writeln!(out, "\n**A software rasterizer measures something else than a GPU.**");
    }
    let _ = writeln!(out);
    let worse = write_table(&mut out, &runs[&Side::Base], &runs[&Side::Candidate], options, comparable, &changed);
    let _ = writeln!(out);
    write_frames(&mut out, &frames);
    Ok((out, worse))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Base,
    Candidate,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Base => "base",
            Side::Candidate => "candidate",
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Checks `commit` out under `work`, lays this checkout's harness over it,
/// and builds it there; when that fails, builds the commit's own harness.
/// Returns the binary, and whether it is the commit's own harness.
fn build_base(root: &Path, work: &Path, commit: &str) -> Result<(PathBuf, bool), String> {
    let tree = work.join("tree");
    if tree.join(".git").exists() {
        git(&tree, &["checkout", "-q", "--force", "--detach", commit])?;
    } else {
        git(root, &["worktree", "prune"])?;
        let tree = tree.to_str().ok_or("a non-UTF-8 path")?;
        git(root, &["worktree", "add", "-q", "--detach", tree, commit])?;
    }
    let manifest = std::fs::read_to_string(tree.join("Cargo.toml")).map_err(|e| e.to_string())?;
    if !manifest.contains("\"tools/bench\"") {
        return Err(format!(
            "{commit} has no tools/bench in its workspace; compare against a commit that has the harness"
        ));
    }
    let harness = tree.join("tools/bench");
    std::fs::remove_dir_all(&harness).map_err(|e| e.to_string())?;
    copy_dir(&root.join("tools/bench"), &harness)?;
    let current = std::fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let bench = std::fs::read_to_string(harness.join("Cargo.toml")).map_err(|e| e.to_string())?;
    std::fs::write(tree.join("Cargo.toml"), with_harness_dependencies(&manifest, &current, &bench))
        .map_err(|e| e.to_string())?;

    eprintln!("mln-bench: building the base ({commit})");
    let bin = work.join("target/release/mln-bench");
    if build(&tree, work)? {
        return Ok((bin, false));
    }
    eprintln!("mln-bench: the base doesn't build this checkout's harness; building its own");
    git(&tree, &["checkout", "-q", "--force", "HEAD", "--", "."])?;
    git(&tree, &["clean", "-q", "-d", "--force", "--", "tools/bench"])?;
    if !build(&tree, work)? {
        return Err("building the base failed, with this checkout's harness and with its own".into());
    }
    Ok((bin, true))
}

/// Builds the harness of the workspace at `tree`; whether it built.
fn build(tree: &Path, work: &Path) -> Result<bool, String> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "mln-rs-bench", "--bin", "mln-bench", "--manifest-path"])
        .arg(tree.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(work.join("target"))
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    Ok(status.success())
}

/// The ref's root manifest, with this checkout's entry of each workspace
/// dependency the harness inherits and the ref lacks, so that the harness
/// can gain dependencies and still build over older commits.
fn with_harness_dependencies(manifest: &str, current: &str, bench: &str) -> String {
    let entry = |text: &str, name: &str| {
        text.lines().find(|line| line.split_once(" = ").is_some_and(|(key, _)| key == name)).map(str::to_string)
    };
    let missing: Vec<String> = bench
        .lines()
        .filter_map(|line| line.strip_suffix(".workspace = true"))
        .filter(|name| entry(manifest, name).is_none())
        .filter_map(|name| entry(current, name))
        .collect();
    if missing.is_empty() {
        return manifest.to_string();
    }
    let header = "[workspace.dependencies]\n";
    manifest.replacen(header, &format!("{header}{}\n", missing.join("\n")), 1)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The cases of this checkout whose code the harness at `tree`, the base's
/// own, doesn't share. A case's code is the `Case::new` call that registers
/// it and the functions and constants it uses, and those they use in turn:
/// its own module's, which it names bare, and another module's that it
/// names through the module (`explore::explore`). They compare as tokens,
/// so formatting and comments don't count. What a case imports from another
/// module, and any type's functions and methods, aren't followed, so the
/// scenes, the runtime and the meter aren't compared; nor are the values a
/// loop around the call passes it.
fn changed_cases(root: &Path, tree: &Path) -> Result<BTreeSet<String>, String> {
    let ours = Code::read(root)?;
    // A base whose harness doesn't read shares no case.
    let theirs = Code::read(tree).ok();
    let mut changed = BTreeSet::new();
    for case in cases::all() {
        let (file, line) = (Path::new(case.registered.file()), case.registered.line() as usize);
        if !theirs.as_ref().is_some_and(|theirs| ours.shares(file, line, theirs)) {
            changed.insert(case.name);
        }
    }
    Ok(changed)
}

/// A harness's functions, constants and `Case::new` calls, as tokens.
#[derive(Default)]
struct Code {
    /// The tokens of each function and constant of a module, by the module's
    /// name and theirs.
    items: BTreeMap<(String, String), Vec<TokenStream>>,
    /// Each `Case::new` call's file, line and tokens.
    calls: Vec<(PathBuf, usize, TokenStream)>,
}

impl Code {
    /// The code of the harness of the workspace at `root`: its `src`'s
    /// files, by their paths from `root`.
    fn read(root: &Path) -> Result<Code, String> {
        let mut code = Code::default();
        let mut directories = vec![root.join("tools/bench/src")];
        while let Some(directory) = directories.pop() {
            let mut paths = std::fs::read_dir(&directory)
                .and_then(|entries| entries.map(|entry| entry.map(|entry| entry.path())).collect::<Result<Vec<_>, _>>())
                .map_err(|e| format!("{}: {e}", directory.display()))?;
            // In order, so that two harnesses list same-named items alike.
            paths.sort();
            for path in paths {
                if path.is_dir() {
                    directories.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let tokens = text.parse().map_err(|e| format!("{}: {e}", path.display()))?;
                    code.add(path.strip_prefix(root).unwrap_or(&path), tokens);
                }
            }
        }
        Ok(code)
    }

    fn add(&mut self, file: &Path, tokens: TokenStream) {
        self.add_calls(file, tokens.clone());
        self.add_items(&module(file), tokens);
    }

    /// The `Case::new` calls anywhere in `tokens`.
    fn add_calls(&mut self, file: &Path, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        for i in 0..trees.len() {
            match &trees[i..] {
                [TokenTree::Group(group), ..] => self.add_calls(file, group.stream()),
                [
                    TokenTree::Ident(case),
                    TokenTree::Punct(a),
                    TokenTree::Punct(b),
                    TokenTree::Ident(new),
                    TokenTree::Group(_),
                    ..,
                ] if case == "Case" && a.as_char() == ':' && b.as_char() == ':' && new == "new" => {
                    let call = trees[i..i + 5].iter().cloned().collect();
                    self.calls.push((file.to_path_buf(), case.span().start().line, call));
                }
                _ => {}
            }
        }
    }

    /// The functions and constants of `module`, `tokens` at its top level,
    /// and of its inline modules; not an `impl`'s, nor the ones inside a
    /// function.
    fn add_items(&mut self, module: &str, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let body = |tree: &TokenTree| matches!(tree, TokenTree::Group(group) if group.delimiter() == Delimiter::Brace);
        let semicolon = |tree: &TokenTree| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ';');
        for i in 0..trees.len() {
            let (end, name) = match &trees[i..] {
                [TokenTree::Ident(keyword), TokenTree::Ident(inner), TokenTree::Group(group), ..]
                    if keyword == "mod" && group.delimiter() == Delimiter::Brace =>
                {
                    self.add_items(&inner.to_string(), group.stream());
                    continue;
                }
                // From `fn` to the end of the body.
                [TokenTree::Ident(keyword), TokenTree::Ident(name), rest @ ..] if keyword == "fn" => {
                    (rest.iter().position(|tree| body(tree) || semicolon(tree)).filter(|&end| body(&rest[end])), name)
                }
                // From the keyword to the `;`; not a `const fn` or a
                // `static mut`.
                [TokenTree::Ident(keyword), TokenTree::Ident(name), rest @ ..]
                    if (keyword == "const" || keyword == "static") && name != "fn" && name != "mut" =>
                {
                    (rest.iter().position(semicolon), name)
                }
                _ => continue,
            };
            if let Some(end) = end {
                let item = trees[i..i + 3 + end].iter().cloned().collect();
                self.items.entry((module.to_string(), name.to_string())).or_default().push(item);
            }
        }
    }

    /// Whether `theirs` has the `Case::new` call at `line` of `file` here,
    /// and what it uses here.
    fn shares(&self, file: &Path, line: usize, theirs: &Code) -> bool {
        let Some((_, _, call)) = self.calls.iter().find(|(at, at_line, _)| at == file && *at_line == line) else {
            return false;
        };
        let Some((their_file, _, _)) = theirs.calls.iter().find(|(_, _, other)| other.to_string() == call.to_string())
        else {
            return false;
        };
        theirs.uses(&module(their_file), call) == self.uses(&module(file), call)
    }

    /// The tokens of the items that `tokens`, in `module`, uses, by their
    /// module's name and theirs.
    fn uses(&self, module: &str, tokens: &TokenStream) -> BTreeMap<(String, String), String> {
        let mut used = BTreeMap::new();
        let mut queue = vec![(module.to_string(), tokens.clone())];
        while let Some((module, tokens)) = queue.pop() {
            for (through, name) in references(tokens) {
                let key = (through.unwrap_or_else(|| module.clone()), name);
                if let Some(items) = self.items.get(&key)
                    && !used.contains_key(&key)
                {
                    queue.extend(items.iter().map(|item| (key.0.clone(), item.clone())));
                    used.insert(key, items.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"));
                }
            }
        }
        used
    }
}

/// The module of the harness's `file`: its name, or its directory's for a
/// `mod.rs`.
fn module(file: &Path) -> String {
    let stem = file.file_stem().unwrap_or_default();
    let name = if stem == "mod" { file.parent().and_then(Path::file_name).unwrap_or_default() } else { stem };
    name.to_string_lossy().into_owned()
}

/// The names that `tokens` refers to, each with the module it names it
/// through, if any: every identifier but one after a dot, a method's or a
/// field's, or after a type in a path (`Scene::new`).
fn references(tokens: TokenStream) -> Vec<(Option<String>, String)> {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let punct = |tree: &TokenTree, c| matches!(tree, TokenTree::Punct(punct) if punct.as_char() == c);
    let mut names = Vec::new();
    for (i, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(group) => names.extend(references(group.stream())),
            TokenTree::Ident(ident) => {
                let module = match &trees[..i] {
                    [.., TokenTree::Ident(module), a, b]
                        if punct(a, ':') && punct(b, ':') && module.to_string().starts_with(char::is_lowercase) =>
                    {
                        Some(module.to_string())
                    }
                    // A method or a field (but not `..rest`), or a type's
                    // item.
                    [.., a, b] if (punct(b, '.') && !punct(a, '.')) || (punct(a, ':') && punct(b, ':')) => continue,
                    _ => None,
                };
                names.push((module, ident.to_string()));
            }
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
    names
}

fn run(bin: &Path, options: &Options, frames: Option<&Path>) -> Result<Report, String> {
    let mut command = Command::new(bin);
    command.args(["run", "--json", "--fixtures"]).arg(&options.fixtures).args(&options.run_args);
    if let Some(frames) = frames {
        command.arg("--frames").arg(frames);
    }
    let output = command.stderr(Stdio::inherit()).output().map_err(|e| format!("{}: {e}", bin.display()))?;
    if !output.status.success() {
        return Err(format!("{} failed ({})", bin.display(), output.status));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("{}'s report: {e}", bin.display()))
}

/// The 1-minute load average, where /proc has it.
fn load_average() -> Option<String> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next().map(str::to_string)
}

/// Each run's value of each metric of `case`, by metric, in the order of
/// the first run.
fn samples(reports: &[Report], case: &str) -> Vec<(String, bool, Vec<f64>)> {
    let metrics: Vec<&Metrics> =
        reports.iter().filter_map(|report| report.cases.iter().find(|c| c.name == case)).map(|c| &c.metrics).collect();
    let Some(first) = metrics.first() else {
        return Vec::new();
    };
    first
        .0
        .iter()
        .map(|metric| {
            let values = metrics.iter().filter_map(|m| m.get(&metric.name)).map(|m| m.value).collect();
            (metric.name.clone(), metric.lead, values)
        })
        .collect()
}

/// Writes the verdicts' table, and returns the case and metric of each
/// worse one.
fn write_table(
    out: &mut String,
    base: &[Report],
    candidate: &[Report],
    options: &Options,
    comparable: bool,
    changed: &BTreeSet<String>,
) -> Vec<(String, String)> {
    let _ = writeln!(out, "| case | metric | base | candidate | change | p | verdict |");
    let _ = writeln!(out, "|---|---|--:|--:|--:|--:|---|");
    let (mut better, mut worse) = (0, Vec::new());
    for case in candidate[0].cases.iter().map(|c| c.name.as_str()) {
        let base_samples: BTreeMap<String, Vec<f64>> =
            samples(base, case).into_iter().map(|(name, _, values)| (name, values)).collect();
        let rows: Vec<_> = samples(candidate, case)
            .into_iter()
            .filter_map(|(name, lead, values)| Some((base_samples.get(&name)?.clone(), name, lead, values)))
            .collect();
        if rows.is_empty() {
            let _ = writeln!(out, "| {case} | only in the candidate | | | | | |");
            continue;
        }
        let lead_p: Vec<f64> = rows.iter().filter(|row| row.2).map(|(b, _, _, c)| mann_whitney(b, c)).collect();
        let mut adjusted = holm(&lead_p).into_iter();
        for (base_values, name, lead, candidate_values) in rows {
            let (b, c) = (median(&base_values), median(&candidate_values));
            let change = if b == 0.0 { if c == 0.0 { 0.0 } else { f64::INFINITY } } else { 100.0 * (c - b) / b };
            let p = if lead { adjusted.next() } else { None };
            if !lead && !options.verbose {
                continue;
            }
            let verdict = match p {
                Some(_) if changed.contains(case) => "code changed",
                Some(p) if comparable && p < ALPHA && change.abs() >= options.threshold => {
                    if change < 0.0 {
                        better += 1;
                        "better"
                    } else {
                        worse.push((case.to_string(), name.clone()));
                        "**worse**"
                    }
                }
                _ => "",
            };
            let p = p.map(|p| format!("{p:.3}")).unwrap_or_default();
            let _ =
                writeln!(out, "| {case} | {name} | {} | {} | {change:+.1}% | {p} | {verdict} |", number(b), number(c));
        }
    }
    let _ = writeln!(out, "\n{better} better, {} worse.", worse.len());
    worse
}

fn number(value: f64) -> String {
    match value.abs() {
        v if v >= 100.0 => format!("{value:.0}"),
        v if v >= 10.0 => format!("{value:.1}"),
        _ => format!("{value:.3}"),
    }
}

/// How many pixels of each last frame differ between the sides, by the
/// render tests' pixelmatch threshold; the differences go to `diff/`.
fn write_frames(out: &mut String, frames: &Path) {
    let (base, candidate, diff) = (frames.join("base"), frames.join("candidate"), frames.join("diff"));
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "a frame is a file the benchmark wrote as `<name>.png`; other names, in any case, are not frames"
    )]
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with(".png"))
        .collect();
    names.sort();
    let _ = writeln!(out, "Last frames ({}):\n", diff.display());
    for name in names {
        let read = |dir: &Path| {
            let bytes = std::fs::read(dir.join(&name)).map_err(|e| e.to_string())?;
            mln_image::decode_png(&bytes).map_err(|e| e.message)
        };
        let line = match (read(&base), read(&candidate)) {
            (Ok(a), Ok(b)) if a.size == b.size => {
                let (width, height) = (a.size.width as usize, a.size.height as usize);
                let mut output = vec![0u8; a.data.len()];
                let different =
                    pixelmatch::pixelmatch(&a.data, &b.data, width, height, Some(&mut output), 0.1285, false);
                if different > 0 {
                    let image = mln_util::image::PremultipliedImage::from_vec(a.size, output);
                    crate::scene::write_frame(&diff, name.trim_end_matches(".png"), &image);
                }
                format!("{different} of {} pixels differ", width * height)
            }
            (Ok(_), Ok(_)) => "the sizes differ".to_string(),
            (a, b) => [a.err(), b.err()].into_iter().flatten().collect::<Vec<_>>().join("; "),
        };
        let _ = writeln!(out, "- {name}: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A harness's cases: A uses a function and a constant of its module and
    /// a function it imports, B a type's function and a method, and C another
    /// module's function.
    const CASES: &str = r#"
use crate::explore::shared;

pub fn all() -> Vec<Case> {
    let mut cases = Vec::new();
    cases.push(Case::new("A", a));
    cases.push(Case::new("B", |cx| b(cx, 2)));
    cases.push(Case::new("C", explore::c));
    cases
}

fn a(cx: &Context) -> Metrics {
    helper(cx, LIMIT)
}

fn helper(cx: &Context, limit: usize) -> Metrics {
    measure(limit + shared())
}

const LIMIT: usize = 10;

fn b(cx: &Context, times: usize) -> Metrics {
    Scene::new().draw(times)
}
"#;

    /// Another module: the scene's and C's.
    const EXPLORE: &str = r"
impl Scene {
    fn new() -> Scene {
        Scene { frames: 1 }
    }

    fn draw(&self, times: usize) -> Metrics {
        measure(times * self.frames)
    }
}

pub(crate) fn c(cx: &Context) -> Metrics {
    measure(3)
}

pub(crate) fn shared() -> usize {
    4
}
";

    const NONE: [&str; 0] = [];

    /// The cases of `CASES` and `EXPLORE` whose code `cases` and `explore`
    /// don't share.
    fn changed(cases: &str, explore: &str) -> Vec<&'static str> {
        let code = |cases: &str, explore: &str| {
            let mut code = Code::default();
            code.add(Path::new("src/cases.rs"), cases.parse().unwrap());
            code.add(Path::new("src/explore.rs"), explore.parse().unwrap());
            code
        };
        let (ours, theirs) = (code(CASES, EXPLORE), code(cases, explore));
        let line = |case: &str| CASES.lines().position(|line| line.contains(&format!("\"{case}\""))).unwrap() + 1;
        ["A", "B", "C"]
            .into_iter()
            .filter(|case| !ours.shares(Path::new("src/cases.rs"), line(case), &theirs))
            .collect()
    }

    #[test]
    fn a_case_changes_with_its_call_and_what_it_uses() {
        assert_eq!(changed(CASES, EXPLORE), NONE);
        let reformatted = CASES
            .replace("    helper(cx, LIMIT)", "    // The helper.\n    helper(\n        cx,\n        LIMIT\n    )");
        assert_eq!(changed(&reformatted, EXPLORE), NONE);
        assert_eq!(changed(&CASES.replace("b(cx, 2)", "b(cx, 3)"), EXPLORE), ["B"]);
        assert_eq!(changed(&CASES.replace("limit + shared()", "limit + 1 + shared()"), EXPLORE), ["A"]);
        assert_eq!(changed(&CASES.replace("LIMIT: usize = 10", "LIMIT: usize = 20"), EXPLORE), ["A"]);
        assert_eq!(changed(CASES, &EXPLORE.replace("measure(3)", "measure(4)")), ["C"]);
        assert_eq!(changed(&CASES.replace("    cases.push(Case::new(\"C\", explore::c));\n", ""), EXPLORE), ["C"]);
    }

    #[test]
    fn imports_methods_and_a_types_functions_are_not_followed() {
        assert_eq!(changed(CASES, &EXPLORE.replace("measure(times * self.frames)", "measure(times)")), NONE);
        assert_eq!(changed(CASES, &EXPLORE.replace("Scene { frames: 1 }", "Scene { frames: 2 }")), NONE);
        assert_eq!(changed(CASES, &EXPLORE.replace("    4\n", "    5\n")), NONE);
    }
}
