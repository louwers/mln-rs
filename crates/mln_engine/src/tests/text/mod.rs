// Ported gtests of test/text/*.test.cpp that need the glyph store or symbol
// layout (GlyphStore, SymbolLayout), mln_style's Formatted, or the text
// fixtures (GlyphCache), one module per C++ file. The others are ported next
// to their code: bidi, tagged_string, glyph_pbf and shaping in crate::text;
// quads, get_anchors and cross_tile_index in crate::symbol;
// local_glyph_rasterizer in render_test (it renders a map).

mod calculate_tile_distances;
mod formatted;
mod glyph_cache;
mod glyph_store;
mod glyph_store_bitmaps;

use std::path::PathBuf;

/// A path of the C++ repository, in the copy of its files these tests read
/// (testdata/).
#[must_use]
pub(crate) fn cpp_path(relative: &str) -> PathBuf {
    std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative)
}

/// Reads a C++ test fixture.
#[must_use]
pub(crate) fn read_file(relative: &str) -> Vec<u8> {
    let path = cpp_path(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}
