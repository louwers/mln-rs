//! The variants a program is tested with: sets of the feature flags its WESL
//! tests.

use std::collections::BTreeSet;

use mln_gfx::shaders::shader_source::BuiltIn;

use crate::{SOURCES, Target, module_of};

/// With more flags than this, [`variants`] are the flags none, all, each
/// alone and all but each, not every subset.
pub const MAX_EXHAUSTIVE_FLAGS: usize = 10;

/// The module `path` names, the longest prefix of an import's path that is a
/// module.
fn imported_module(path: &str) -> Option<&'static str> {
    let parts: Vec<&str> = path.split("::").collect();
    (1..=parts.len()).rev().find_map(|len| {
        let name = parts[..len].join("::");
        SOURCES.iter().find(|&&(module, _)| module == name).map(|&(module, _)| module)
    })
}

/// The feature flags, other than the targets', that `program`'s WESL and the
/// modules it imports test with `@if` and `@elif`, in order; none for a
/// program without WESL.
#[must_use]
pub fn flags_of(program: BuiltIn) -> Vec<String> {
    let mut flags = BTreeSet::new();
    let mut pending: Vec<&str> = module_of(program).into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(module) = pending.pop() {
        if !seen.insert(module) {
            continue;
        }
        let Some(&(_, source)) = SOURCES.iter().find(|&&(name, _)| name == module) else { continue };
        for line in source.lines() {
            if let Some(path) = line.trim_start().strip_prefix("import package::") {
                let path = path.split(['{', ';']).next().unwrap_or_default().trim_end_matches("::");
                pending.extend(imported_module(path));
            }
        }
        let conditions = ["@if(", "@elif("].into_iter().flat_map(|attribute| source.split(attribute).skip(1));
        for condition in conditions {
            let condition = condition.split(')').next().unwrap_or_default();
            for word in condition.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if !word.is_empty() && !matches!(word, "true" | "false") {
                    flags.insert(word.to_owned());
                }
            }
        }
    }
    for target in Target::ALL {
        flags.remove(target.feature());
    }
    flags.into_iter().collect()
}

/// The sets of `flags` to test a program with: every subset, or with more
/// than `max_exhaustive` flags, none, all, each alone and all but each.
#[must_use]
pub fn variants(flags: &[String], max_exhaustive: usize) -> Vec<Vec<String>> {
    let subset = |mask: u64| -> Vec<String> {
        flags.iter().enumerate().filter(|&(i, _)| mask & (1 << i) != 0).map(|(_, flag)| flag.clone()).collect()
    };
    let all = if flags.len() >= 64 { u64::MAX } else { (1u64 << flags.len()) - 1 };
    if flags.len() <= max_exhaustive {
        return (0..=all).map(subset).collect();
    }
    let mut masks = vec![0, all];
    for i in 0..flags.len().min(64) {
        masks.extend([1 << i, all & !(1 << i)]);
    }
    masks.sort_unstable();
    masks.dedup();
    masks.into_iter().map(subset).collect()
}
