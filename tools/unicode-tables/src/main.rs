//! `unicode-tables arabic-shaping <ArabicShaping.txt> <UnicodeData.txt>` and
//! `unicode-tables bidi-mirroring <BidiMirroring.txt>`: write the tables that
//! `mln_text` reads from the Unicode Character Database,
//! `crates/mln_text/src/arabic_tables.rs` and `bidi_mirroring.rs`.
//! `mise run generate:arabic-shaping` and `generate:bidi-mirroring` download
//! the files and run it.
//!
//! The tables store UTF-16 code units, so every character in them must be in
//! the BMP.

mod arabic_shaping;
mod bidi_mirroring;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: unicode-tables arabic-shaping <ArabicShaping.txt> <UnicodeData.txt>
       unicode-tables bidi-mirroring <BidiMirroring.txt>";

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
    let (file, table) = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["arabic-shaping", arabic_shaping, unicode_data] => {
            ("arabic_tables.rs", arabic_shaping::table(&read(arabic_shaping)?, &read(unicode_data)?)?)
        }
        ["bidi-mirroring", bidi_mirroring] => ("bidi_mirroring.rs", bidi_mirroring::table(&read(bidi_mirroring)?)?),
        _ => return Err(USAGE.into()),
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/mln_text/src").join(file);
    std::fs::write(&path, table).map_err(|error| format!("{}: {error}", path.display()))
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))
}

/// The version that a UCD file's header line names: `# <name>-<version>.txt`.
fn ucd_version<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let version = line.strip_prefix("# ")?.strip_prefix(name)?.strip_prefix('-')?.strip_suffix(".txt")?;
        (!version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')).then_some(version)
    })
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
    u32::from_str_radix(field.trim(), 16).map_err(|_| format!("{field:?} isn't a code point"))
}

/// The character as the tables store it.
fn bmp(c: u32) -> Result<u16, String> {
    u16::try_from(c).map_err(|_| format!("U+{c:04X} is outside the BMP"))
}
