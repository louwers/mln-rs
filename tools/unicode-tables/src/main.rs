//! `unicode-tables`: writes the tables that `mln_text` reads from the Unicode
//! Character Database, `crates/mln_text/src/arabic_tables.rs` and
//! `bidi_mirroring.rs`, from the UCD files in `ucd/`. `mise run
//! generate:unicode-tables` downloads the files and runs it.
//!
//! The files must be of unicode-bidi's Unicode version, which
//! `unicode-tables --unicode-version` prints, so that the characters
//! unicode-bidi resolves as right to left and the ones that the tables mirror
//! and shape come from one version. The tables store UTF-16 code units, so
//! every character in them must be in the BMP.

mod arabic_shaping;
mod bidi_mirroring;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: unicode-tables [--unicode-version]

--unicode-version prints the Unicode version that the UCD files must be of.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("unicode-tables: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => {}
        [flag] if flag == "--unicode-version" => {
            println!("{}", unicode_version());
            return Ok(());
        }
        _ => return Err(USAGE.into()),
    }
    for (path, table) in tables(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))? {
        std::fs::write(&path, table).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// Each table and the path of its file, in the workspace at `root`.
fn tables(root: &Path) -> Result<[(PathBuf, String); 2], String> {
    let read = |name: &str| {
        let path = root.join("tools/unicode-tables/ucd").join(name);
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let out = root.join("crates/mln_text/src");
    Ok([
        (out.join("arabic_tables.rs"), arabic_shaping::table(&read("ArabicShaping.txt")?, &read("UnicodeData.txt")?)?),
        (out.join("bidi_mirroring.rs"), bidi_mirroring::table(&read("BidiMirroring.txt")?)?),
    ])
}

/// unicode-bidi's Unicode version, which the UCD files must be of.
fn unicode_version() -> String {
    let (major, minor, update) = unicode_bidi::UNICODE_VERSION;
    format!("{major}.{minor}.{update}")
}

/// The version that a UCD file's header line names, `# <name>-<version>.txt`,
/// if it's unicode-bidi's.
fn ucd_version(text: &str, name: &str) -> Result<String, String> {
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("# ")?.strip_prefix(name)?.strip_prefix('-')?.strip_suffix(".txt"))
        .ok_or_else(|| format!("not a {name}.txt"))?;
    let expected = unicode_version();
    if version != expected {
        return Err(format!("{name}.txt is of Unicode {version}, not unicode-bidi's {expected}"));
    }
    Ok(expected)
}

/// The fields of each line of a UCD file that holds data, without comments.
fn ucd_rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines().filter_map(|line| {
        let data = line.split_once('#').map_or(line, |(data, _)| data).trim();
        (!data.is_empty()).then(|| data.split(';').map(str::trim).collect())
    })
}

/// A code point that a UCD file writes in hexadecimal.
fn code_point(field: &str) -> Result<u32, String> {
    u32::from_str_radix(field, 16).map_err(|_| format!("{field:?} isn't a code point"))
}

/// The character as the tables store it.
fn bmp(c: u32) -> Result<u16, String> {
    u16::try_from(c).map_err(|_| format!("U+{c:04X} is outside the BMP"))
}
