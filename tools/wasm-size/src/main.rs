//! `wasm-size <module> [--base <module>] [--top <n>] [--html <path>]`: how
//! many bytes of a wasm module each crate and function takes, or how they
//! changed from a base module, as text for a terminal and optionally as a
//! treemap page (`mise run web:size`).
//!
//! The module needs its name section, which rustc writes and wasm-opt keeps
//! with `-g`. rustc writes demangled names there, with each crate's
//! disambiguator (`mln_util[0123abcd]::geo::Point`). The report drops the
//! disambiguators, which differ between checkouts, so that functions match
//! across builds. A function counts toward the first crate in its name that
//! isn't the standard library, so `core::ptr::drop_in_place::<mln_map::Map>`
//! counts toward `mln_map`. Code that was inlined counts toward its caller,
//! and data segments, which hold every crate's constants, count as one item.
//! The name section, which the shipped module doesn't have, is left out.

mod text;
mod treemap;

use std::collections::BTreeMap;
use std::io::{IsTerminal as _, Write as _};
use std::ops::Range;
use std::path::Path;
use std::process::ExitCode;

use wasmparser::{BinaryReader, Name, NameSectionReader, Parser, Payload, TypeRef};

/// Crates whose generic code counts toward the crate it's instantiated for.
const STD_CRATES: &[&str] = &["alloc", "compiler_builtins", "core", "dlmalloc", "hashbrown", "std"];

const USAGE: &str = "usage: wasm-size <module> [--base <module>] [--top <n>] [--html <path>]

Reads COLUMNS for the width to cut names to, and colors changes on a terminal
unless NO_COLOR is set.";

fn main() -> ExitCode {
    match run() {
        Ok(report) => {
            // A closed pipe (`| head`) isn't an error.
            let _ = std::io::stdout().write_all(report.as_bytes());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("wasm-size: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let mut module = None;
    let mut base = None;
    let mut html = None;
    let mut top = 20;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--base" => base = Some(args.next().ok_or(USAGE)?),
            "--html" => html = Some(args.next().ok_or(USAGE)?),
            "--top" => top = args.next().and_then(|n| n.parse().ok()).ok_or(USAGE)?,
            "-h" | "--help" => return Ok(format!("{USAGE}\n")),
            _ if module.is_none() && !arg.starts_with('-') => module = Some(arg),
            _ => return Err(USAGE.into()),
        }
    }
    let module = module.ok_or(USAGE)?;
    let title = Path::new(&module).file_name().map_or(module.clone(), |name| name.to_string_lossy().into_owned());
    let sizes = Sizes::read(&module)?;
    let base = base.map(|base| Sizes::read(&base)).transpose()?;
    if let Some(path) = html {
        std::fs::write(&path, treemap::page(&title, &sizes, base.as_ref())).map_err(|e| format!("{path}: {e}"))?;
        eprintln!("wasm-size: wrote {path}");
    }
    let style = text::Style {
        width: std::env::var("COLUMNS").ok().and_then(|columns| columns.parse().ok()),
        color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
    };
    Ok(match &base {
        Some(base) => text::compare(&title, base, &sizes, top, style),
        None => text::report(&title, &sizes, top, style),
    })
}

/// A module's bytes by item.
#[derive(Debug, Default)]
struct Sizes {
    /// The module's size, without its name section.
    total: u64,
    /// Each function name, the data segments and each other section.
    /// Functions of the same name, closures among them, add up.
    items: BTreeMap<String, Item>,
}

#[derive(Debug, Default)]
struct Item {
    bytes: u64,
    /// The crate a function counts toward; `None` for an item that isn't a
    /// named function, which is a group of its own.
    krate: Option<String>,
}

impl Item {
    fn group<'a>(&'a self, name: &'a str) -> &'a str {
        self.krate.as_deref().unwrap_or(name)
    }
}

impl Sizes {
    fn read(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        Self::parse(&bytes).map_err(|e| format!("{path}: {e}"))
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut imported_functions = 0;
        let mut bodies = Vec::new();
        let mut names = BTreeMap::new();
        let mut sizes = Sizes { total: bytes.len() as u64, ..Sizes::default() };
        for payload in Parser::new(0).parse_all(bytes) {
            match payload.map_err(|e| e.to_string())? {
                Payload::ImportSection(imports) => {
                    for import in imports.into_imports() {
                        if matches!(import.map_err(|e| e.to_string())?.ty, TypeRef::Func(_)) {
                            imported_functions += 1;
                        }
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let index = imported_functions + bodies.len() as u32;
                    bodies.push((index, len(body.range())));
                }
                Payload::DataSection(data) => {
                    for segment in data {
                        sizes.add("(data segments)", None, segment.map_err(|e| e.to_string())?.data.len() as u64);
                    }
                }
                Payload::CustomSection(section) if section.name() == "name" => {
                    sizes.total -= len(section.range());
                    let reader = BinaryReader::new(section.data(), section.data_offset());
                    for subsection in NameSectionReader::new(reader) {
                        if let Name::Function(map) = subsection.map_err(|e| e.to_string())? {
                            for naming in map {
                                let naming = naming.map_err(|e| e.to_string())?;
                                names.insert(naming.index, naming.name);
                            }
                        }
                    }
                }
                Payload::CustomSection(section) => {
                    sizes.add(&format!("(custom section {})", section.name()), None, len(section.range()));
                }
                _ => {}
            }
        }
        if names.is_empty() && !bodies.is_empty() {
            return Err("no function names: build it without stripping them (wasm-opt -g)".into());
        }
        for (index, size) in bodies {
            match names.get(&index) {
                Some(name) => {
                    let (name, krate) = without_disambiguators(name);
                    sizes.add(&name, Some(krate.unwrap_or("(other functions)")), size);
                }
                None => sizes.add("(unnamed functions)", None, size),
            }
        }
        let counted: u64 = sizes.items.values().map(|item| item.bytes).sum();
        sizes.add("(other sections)", None, sizes.total - counted);
        Ok(sizes)
    }

    fn add(&mut self, name: &str, krate: Option<&str>, bytes: u64) {
        let item = self.items.entry(name.to_owned()).or_default();
        item.bytes += bytes;
        item.krate = krate.map(str::to_owned);
    }

    /// The bytes of each crate, and of each item that isn't a named function.
    fn groups(&self) -> BTreeMap<&str, u64> {
        let mut groups = BTreeMap::new();
        for (name, item) in &self.items {
            *groups.entry(item.group(name)).or_default() += item.bytes;
        }
        groups
    }

    /// The bytes of each named function.
    fn functions(&self) -> BTreeMap<&str, u64> {
        self.items
            .iter()
            .filter(|(_, item)| item.krate.is_some())
            .map(|(name, item)| (name.as_str(), item.bytes))
            .collect()
    }
}

fn len(range: Range<u64>) -> u64 {
    range.end - range.start
}

/// The name without the crates' disambiguators, and the crate it counts
/// toward.
fn without_disambiguators(name: &str) -> (String, Option<&str>) {
    let mut out = String::with_capacity(name.len());
    let mut crates = Vec::new();
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        let hash = rest[open + 1..].split(']').next().filter(|hash| {
            rest[open + 1..].contains(']') && !hash.is_empty() && hash.bytes().all(|b| b.is_ascii_hexdigit())
        });
        let ident_start = rest[..open].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        match hash {
            Some(hash) if ident_start < open => {
                crates.push(&rest[ident_start..open]);
                out.push_str(&rest[..open]);
                rest = &rest[open + hash.len() + 2..];
            }
            _ => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    let krate = crates.iter().find(|krate| !STD_CRATES.contains(krate)).or(crates.first()).copied();
    (out, krate)
}

/// The index of the `>` that closes an already opened `<`.
pub(crate) fn closing(text: &str) -> Option<usize> {
    let mut depth = 0;
    let bytes = text.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'<' => depth += 1,
            b'>' if i > 0 && bytes[i - 1] == b'-' => {}
            b'>' if depth == 0 => return Some(i),
            b'>' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The index of `needle` outside any brackets.
pub(crate) fn top_level_find(text: &str, needle: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' if i > 0 && bytes[i - 1] == b'-' => {}
            b'>' | b')' | b']' => depth -= 1,
            _ if depth == 0 && text[i..].starts_with(needle) => return Some(i),
            _ => {}
        }
    }
    None
}

/// The entries, largest first.
fn largest<'a>(sizes: &BTreeMap<&'a str, u64>) -> Vec<(&'a str, u64)> {
    let mut entries: Vec<_> = sizes.iter().map(|(name, bytes)| (*name, *bytes)).collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    entries
}

/// The entries whose size changed, as (name, base, size), largest change
/// first; an entry one side lacks has size 0 there.
fn changes<'a>(base: &BTreeMap<&'a str, u64>, sizes: &BTreeMap<&'a str, u64>) -> Vec<(&'a str, u64, u64)> {
    let mut names: Vec<&str> = base.keys().chain(sizes.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    let mut changes: Vec<_> = names
        .into_iter()
        .map(|name| (name, base.get(name).copied().unwrap_or(0), sizes.get(name).copied().unwrap_or(0)))
        .filter(|(_, old, new)| old != new)
        .collect();
    changes.sort_by_key(|(_, old, new)| std::cmp::Reverse(new.abs_diff(*old)));
    changes
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A module importing one function and defining two, of `bodies` no-ops
    /// each, with a 5-byte data segment, and with their names when `named`.
    pub(crate) fn module(bodies: [usize; 2], named: bool) -> Vec<u8> {
        fn section(id: u8, payload: &[u8]) -> Vec<u8> {
            let mut out = vec![id, u8::try_from(payload.len()).expect("a short section")];
            out.extend_from_slice(payload);
            out
        }
        fn name(text: &str) -> Vec<u8> {
            let mut out = vec![u8::try_from(text.len()).expect("a short name")];
            out.extend_from_slice(text.as_bytes());
            out
        }
        let mut out = b"\0asm\x01\0\0\0".to_vec();
        out.extend(section(1, &[1, 0x60, 0, 0]));
        out.extend(section(2, &[[1].as_slice(), &name("m"), &name("f"), &[0, 0]].concat()));
        out.extend(section(3, &[2, 0, 0]));
        out.extend(section(5, &[1, 0, 1]));
        let code: Vec<u8> = bodies
            .iter()
            .flat_map(|&nops| {
                [vec![u8::try_from(nops + 2).expect("a short body"), 0], vec![1; nops], vec![0x0b]].concat()
            })
            .collect();
        out.extend(section(10, &[[2].as_slice(), &code].concat()));
        out.extend(section(11, &[1, 0, 0x41, 0, 0x0b, 5, 1, 2, 3, 4, 5]));
        if named {
            let names = [
                [3].as_slice(),
                &[0],
                &name("m::f"),
                &[1],
                &name("mln_util[0123abcd]::f"),
                &[2],
                &name("core[0011aabb]::ptr::drop_glue::<mln_map[abcdef01]::Map>"),
            ]
            .concat();
            out.extend(section(0, &[name("name"), section(1, &names)].concat()));
        }
        out
    }

    #[test]
    fn reads_function_sizes_past_the_imports() {
        let bytes = module([10, 4], true);
        let sizes = Sizes::parse(&bytes).unwrap();
        assert_eq!(
            sizes.functions(),
            BTreeMap::from([("core::ptr::drop_glue::<mln_map::Map>", 6), ("mln_util::f", 12)])
        );
        let groups = sizes.groups();
        assert_eq!((groups["mln_util"], groups["mln_map"], groups["(data segments)"]), (12, 6, 5));
        assert_eq!(groups.values().sum::<u64>(), sizes.total, "the groups add up to the module");
        assert!(sizes.total < bytes.len() as u64, "the name section is left out");
    }

    #[test]
    fn a_module_without_names_is_an_error() {
        assert!(Sizes::parse(&module([10, 4], false)).unwrap_err().contains("no function names"));
    }

    #[test]
    fn drops_disambiguators_and_finds_the_crate() {
        assert_eq!(
            without_disambiguators("core[37ee0ce0]::ptr::drop_glue::<mln_map[b15d6d7f]::map::Map>"),
            ("core::ptr::drop_glue::<mln_map::map::Map>".into(), Some("mln_map"))
        );
        assert_eq!(
            without_disambiguators("<alloc[42d2b33b]::vec::Vec<u8>>::push"),
            ("<alloc::vec::Vec<u8>>::push".into(), Some("alloc"))
        );
        assert_eq!(without_disambiguators("decode::<[u8; 4], 256usize>"), ("decode::<[u8; 4], 256usize>".into(), None));
        assert_eq!(without_disambiguators("__wbindgen_malloc"), ("__wbindgen_malloc".into(), None));
    }

    #[test]
    fn changes_put_the_largest_first() {
        let base = BTreeMap::from([("a", 10), ("b", 100), ("c", 5)]);
        let sizes = BTreeMap::from([("a", 50), ("b", 100), ("d", 1)]);
        assert_eq!(changes(&base, &sizes), [("a", 10, 50), ("c", 5, 0), ("d", 0, 1)]);
    }
}
