//! `unicode-tables <directory>`: writes the tables that `mln_engine` reads from
//! the Unicode Character Database, `crates/mln_engine/src/text/arabic_tables.rs` and
//! `bidi_mirroring.rs`, from the directory's `ArabicShaping.txt`,
//! `UnicodeData.txt` and `BidiMirroring.txt`.
//! `mise run generate:unicode-tables` runs it on the files that `mise deps`
//! downloads.
//!
//! The files must be of unicode-bidi's Unicode version, which
//! `.config/mise/config.toml`'s `unicode_version` names, so that the characters
//! unicode-bidi resolves as right to left and the ones that the tables mirror
//! and shape come from one version. The tables store UTF-16 code units, so
//! every character in them must be in the BMP.

mod arabic_shaping;
mod bidi_mirroring;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: unicode-tables <directory>

<directory> holds the UCD's ArabicShaping.txt, UnicodeData.txt and
BidiMirroring.txt, of unicode-bidi's Unicode version.";

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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let directory = match &args[..] {
        [directory] if !directory.starts_with('-') => Path::new(directory),
        _ => return Err(USAGE.into()),
    };
    let read = |name: &str| {
        let path = directory.join(name);
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let arabic_tables = arabic_shaping::table(&read("ArabicShaping.txt")?, &read("UnicodeData.txt")?)?;
    let bidi_mirroring = bidi_mirroring::table(&read("BidiMirroring.txt")?)?;
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mln_engine/src/text");
    for (file, table) in [("arabic_tables.rs", arabic_tables), ("bidi_mirroring.rs", bidi_mirroring)] {
        let path = out.join(file);
        std::fs::write(&path, table).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

/// The version that a UCD file's header line names, `# <name>-<version>.txt`,
/// if it's unicode-bidi's.
fn ucd_version(text: &str, name: &str) -> Result<String, String> {
    let version = text
        .lines()
        .find_map(|line| line.strip_prefix("# ")?.strip_prefix(name)?.strip_prefix('-')?.strip_suffix(".txt"))
        .ok_or_else(|| format!("not a {name}.txt"))?;
    let (major, minor, update) = unicode_bidi::UNICODE_VERSION;
    let expected = format!("{major}.{minor}.{update}");
    if version != expected {
        return Err(format!(
            "{name}.txt is of Unicode {version}, not unicode-bidi's {expected}: set unicode_version in .config/mise/config.toml to it"
        ));
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
