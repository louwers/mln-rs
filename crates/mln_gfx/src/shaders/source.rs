//! The code of a program in each shading language a backend compiles, which
//! a [`Program`](crate::shaders::program::Program) carries in its
//! [`Sources`](crate::shaders::program::Sources): [`wgsl`] for
//! `mln_webgpu`, [`msl`] for `mln_mtl`, [`glsl_es`] for `mln_gl`, and
//! [`glsl`] for the backends that compile through `mln_glsl`, `mln_vulkan`
//! and `mln_d3d12`. Each backend composes a program's code with a prelude
//! of its own, of the helpers the shaders share, and with the declarations
//! of the blocks of the engine's buffers and the program's, which it writes
//! from their [`Block`](crate::shaders::block::Block)s: a source declares
//! no block itself.

pub mod glsl;
pub mod glsl_es;
pub mod msl;
pub mod wgsl;

/// The identifiers that `source`, a shader's code in any of the languages,
/// names outside its `//` and `/* */` comments, and the numbers it writes.
/// A backend declares for a stage the blocks it names.
pub fn identifiers(source: &str) -> impl Iterator<Item = &str> {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut rest = source;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start_matches(|c: char| !word(c) && c != '/');
            if let Some(after) = rest.strip_prefix("//") {
                rest = after.find('\n').map_or("", |end| &after[end..]);
            } else if let Some(after) = rest.strip_prefix("/*") {
                rest = after.find("*/").map_or("", |end| &after[end + 2..]);
            } else if let Some(after) = rest.strip_prefix('/') {
                rest = after;
            } else if rest.is_empty() {
                return None;
            } else {
                let end = rest.find(|c: char| !word(c)).unwrap_or(rest.len());
                let (identifier, after) = rest.split_at(end);
                rest = after;
                return Some(identifier);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn identifiers_skip_comments() {
        let source = "a_b = c / d; // e\n/* f\n g */ h*/";
        assert_eq!(super::identifiers(source).collect::<Vec<_>>(), ["a_b", "c", "d", "h"]);
    }
}
