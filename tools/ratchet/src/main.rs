//! `ratchet [--update]`: fails when a file has more library panic sites or
//! `RefCell` borrows than recorded (`mise run ratchet`).
//!
//! Two counts per Rust file, in the libraries' non-test code (`src/`,
//! `crates/*/src`, `vendor/*/src`, minus `#[cfg(test)]` items and the files of
//! `#[cfg(test)] mod name;` modules):
//!
//! - `panic`: `.unwrap()`, `.expect(`, `panic!`, `unreachable!`, `todo!` and
//!   `unimplemented!`. A C++ exception or invariant ported as a panic takes down
//!   the embedding app: where C++ throws, return the error.
//! - `borrow`: `.borrow()` and `.borrow_mut()`, each a `RefCell` borrow that
//!   panics when it overlaps a conflicting one. The scan can't see types, so a
//!   file that names the `Borrow` or `BorrowMut` trait and never `RefCell` counts
//!   none: its calls are the trait's.
//!
//! `tools/ratchet/baseline.toml` records the counts. A file absent from it allows
//! none, and a count above its baseline fails. A count below its baseline also
//! fails until `--update` lowers it, so the baseline only shrinks. `--update`
//! never raises a count or adds a file: a change that needs more raises the count
//! by hand, where the review sees it. When a file moves, rename its key by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::LazyLock;

use regex::Regex;

const USAGE: &str = "usage: ratchet [--update]

--update lowers the baseline's counts to the current ones.";

/// The rules, in the order of `Counts` and the baseline.
const RULES: [&str; 2] = ["panic", "borrow"];

/// Each rule's count per file, for the files whose count isn't zero.
type Counts = [BTreeMap<String, usize>; RULES.len()];

static LIBRARY_SOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:src/|(?:crates|vendor)/[^/]+/src/)").unwrap());
static PANIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.unwrap\(\)|\.expect\(|\b(?:panic|unreachable|todo|unimplemented)!\s*[(\[{]").unwrap()
});
static BORROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.borrow(?:_mut)?\(\)").unwrap());
static BORROW_TRAIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bBorrow(?:Mut)?\b").unwrap());
static REF_CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bRefCell\b").unwrap());
static TEST_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#\[cfg\((?:all\()?test\b[^\]]*\]\s*").unwrap());
static PATH_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^#\[path\s*=\s*"([^"]+)"\]"#).unwrap());
static MOD_DECLARATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;").unwrap());

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let update = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => false,
        ["--update"] => true,
        ["-h" | "--help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(update) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("ratchet: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Whether every count matches its baseline.
fn run(update: bool) -> Result<bool, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("baseline.toml");
    let current = scan(&root)?;
    let baseline = read_baseline(&baseline_path)?;
    let mut errors = Vec::new();
    let mut lowered = Counts::default();
    for (((rule, found), allowed), lowered) in RULES.iter().zip(&current).zip(&baseline).zip(&mut lowered) {
        *lowered = allowed
            .iter()
            .filter_map(|(path, &limit)| found.get(path).map(|&have| (path.clone(), limit.min(have))))
            .collect();
        for path in allowed.keys().chain(found.keys()).collect::<BTreeSet<_>>() {
            let have = found.get(path).copied().unwrap_or(0);
            let limit = allowed.get(path).copied().unwrap_or(0);
            if have > limit {
                errors.push(format!("{path}: {have} {rule}, baseline {limit}"));
            } else if have < limit && !update {
                errors.push(format!("{path}: {have} {rule}, baseline {limit}; run `mise run ratchet --update`"));
            }
        }
    }
    if update {
        write_baseline(&baseline_path, &lowered)?;
    }
    for error in &errors {
        eprintln!("error: {error}");
    }
    if !errors.is_empty() {
        eprintln!(
            "Return errors instead of panicking in library code, and give state one owner instead of borrowing it \
             from a RefCell (tools/ratchet)."
        );
    }
    let totals: Vec<String> =
        RULES.iter().zip(&current).map(|(rule, found)| format!("{rule}: {}", found.values().sum::<usize>())).collect();
    println!("{}", totals.join(", "));
    Ok(errors.is_empty())
}

/// The counts of the Rust files that Git tracks or would track.
fn scan(root: &Path) -> Result<Counts, String> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "*.rs"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git ls-files: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let files = String::from_utf8(output.stdout).map_err(|error| format!("git ls-files: {error}"))?;
    let mut texts = Vec::new();
    for path in files.split('\0').filter(|path| !path.is_empty() && !path.contains("/testdata/")) {
        match std::fs::read_to_string(root.join(path)) {
            Ok(text) => texts.push((path, text)),
            // Deleted but still in the index, a submodule, or not UTF-8.
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::IsADirectory | ErrorKind::InvalidData) => {}
            Err(error) => return Err(format!("{path}: {error}")),
        }
    }
    let sources: Vec<Source> = texts.iter().map(|(path, text)| Source::new(path, text)).collect();
    let test_prefixes: Vec<String> = sources.iter().flat_map(Source::test_modules).collect();
    let mut result = Counts::default();
    for source in &sources {
        if !LIBRARY_SOURCE.is_match(source.path) || test_prefixes.iter().any(|prefix| source.path.starts_with(prefix)) {
            continue;
        }
        for (found, count) in result.iter_mut().zip(source.counts()) {
            if count > 0 {
                found.insert(source.path.to_owned(), count);
            }
        }
    }
    Ok(result)
}

/// A Rust file, and which of its lines are in `#[cfg(test)]` items.
struct Source<'a> {
    path: &'a str,
    lines: Vec<&'a str>,
    tests: Vec<bool>,
}

impl<'a> Source<'a> {
    fn new(path: &'a str, text: &'a str) -> Self {
        let lines: Vec<&str> = text.lines().collect();
        let tests = test_lines(&lines);
        Self { path, lines, tests }
    }

    /// The counts of the code outside tests, in `RULES`' order.
    fn counts(&self) -> [usize; RULES.len()] {
        let code: Vec<&str> =
            self.lines.iter().zip(&self.tests).filter(|(_, test)| !**test).map(|(line, _)| code_part(line)).collect();
        let panic = code.iter().map(|line| PANIC.find_iter(line).count()).sum();
        let trait_only =
            code.iter().any(|line| BORROW_TRAIT.is_match(line)) && !code.iter().any(|line| REF_CELL.is_match(line));
        let borrow = if trait_only { 0 } else { code.iter().map(|line| BORROW.find_iter(line).count()).sum() };
        [panic, borrow]
    }

    /// Path prefixes of the files of the `#[cfg(test)] mod name;` modules.
    fn test_modules(&self) -> Vec<String> {
        let Some((parent, name)) = self.path.rsplit_once('/') else {
            return Vec::new();
        };
        let directory = match name {
            "lib.rs" | "main.rs" | "mod.rs" => parent.to_owned(),
            _ => format!("{parent}/{}", name.strip_suffix(".rs").unwrap_or(name)),
        };
        let mut prefixes = Vec::new();
        let mut explicit = None;
        for (line, _) in self.lines.iter().zip(&self.tests).filter(|(_, test)| **test) {
            let line = line.trim();
            if let Some(attribute) = PATH_ATTRIBUTE.captures(line) {
                // Relative to the declaring file's directory.
                explicit = Some(format!("{parent}/{}", &attribute[1]));
            }
            if let Some(declaration) = MOD_DECLARATION.captures(line) {
                let module = &declaration[1];
                match explicit.take() {
                    Some(path) => prefixes.push(path),
                    None => prefixes.extend([format!("{directory}/{module}.rs"), format!("{directory}/{module}/")]),
                }
            }
        }
        prefixes
    }
}

/// The line without its `//` comment, skipping `//` inside string literals.
fn code_part(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if in_string => index += 1,
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes.get(index + 1) == Some(&b'/') => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

/// Index past the item at `start`, by counting braces from its first line.
fn block_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0;
    let mut opened = false;
    for (index, line) in lines.iter().enumerate().skip(start) {
        let line = code_part(line);
        if depth == 0 && line.split_once(';').is_some_and(|(before, _)| !before.contains('{')) {
            return index + 1;
        }
        depth += line.matches('{').count().cast_signed() - line.matches('}').count().cast_signed();
        opened |= line.contains('{');
        if depth <= 0 && opened {
            return index + 1;
        }
    }
    lines.len()
}

/// Whether each line is in a `#[cfg(test)]` item.
fn test_lines(lines: &[&str]) -> Vec<bool> {
    let mut tests = vec![false; lines.len()];
    let mut index = 0;
    while index < lines.len() {
        let stripped = lines[index].trim();
        let Some(attribute) = TEST_ATTRIBUTE.find(stripped) else {
            index += 1;
            continue;
        };
        // The gated item follows the attribute on its line or below any
        // further attributes.
        let mut start = if attribute.end() < stripped.len() { index } else { index + 1 };
        while start < lines.len() && lines[start].trim().starts_with("#[") {
            start += 1;
        }
        let end = if start < lines.len() { block_end(lines, start) } else { lines.len() };
        tests[index..end].fill(true);
        index = end;
    }
    tests
}

fn read_baseline(path: &Path) -> Result<Counts, String> {
    let error = |message: String| format!("{}: {message}", path.display());
    let text = std::fs::read_to_string(path).map_err(|e| error(e.to_string()))?;
    let document = toml_edit::Document::parse(text).map_err(|e| error(e.to_string()))?;
    let mut baseline = Counts::default();
    for (rule, counts) in RULES.iter().zip(&mut baseline) {
        let Some(item) = document.get(rule) else {
            continue;
        };
        let table = item.as_table().ok_or_else(|| error(format!("{rule} isn't a table")))?;
        for (file, count) in table {
            let count = count
                .as_integer()
                .and_then(|count| usize::try_from(count).ok())
                .ok_or_else(|| error(format!("{rule}.{file:?} isn't a count")))?;
            counts.insert(file.to_owned(), count);
        }
    }
    Ok(baseline)
}

fn write_baseline(path: &Path, baseline: &Counts) -> Result<(), String> {
    let mut text = String::from(
        "# Library panic sites and RefCell borrows per file (tools/ratchet).\n\
         # Counts only fall: `mise run ratchet --update` lowers them. Raise one by hand\n\
         # in the change that needs it.\n",
    );
    for (rule, counts) in RULES.iter().zip(baseline) {
        let _ = write!(text, "\n[{rule}]\n");
        for (file, count) in counts {
            let _ = writeln!(text, "\"{file}\" = {count}");
        }
    }
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}
