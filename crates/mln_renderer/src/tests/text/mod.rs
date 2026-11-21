// Ported gtests of test/text/*.test.cpp that need mln_renderer (GlyphStore),
// sit above mln_text (Formatted is mln_expression's) or read its fixtures
// (GlyphCache), one module per C++ file. The others are ported next to their
// code: bidi, tagged_string, glyph_pbf and shaping in mln_text;
// calculate_tile_distances, quads, get_anchors and cross_tile_symbol_index
// in mln_layers' symbol layer type; local_glyph_rasterizer in render_test
// (it renders a map).

mod formatted;
mod glyph_cache;
mod glyph_store;
mod glyph_store_bitmaps;

use std::path::PathBuf;

use mln_util::logging::{Event, EventSeverity, Log, Observer};

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

struct NullObserver;

impl Observer for NullObserver {
    fn on_record(&self, _: EventSeverity, _: Event, _: i64, _: &str) -> bool {
        true
    }
}

/// Silences the calling thread's log until dropped.
pub(crate) struct SquelchLog;

impl SquelchLog {
    #[must_use]
    pub(crate) fn new() -> Self {
        let _ = Log::set_thread_observer(Some(Box::new(NullObserver)));
        SquelchLog
    }
}

impl Default for SquelchLog {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SquelchLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}
