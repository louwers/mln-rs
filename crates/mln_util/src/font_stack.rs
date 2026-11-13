// Ports include/mln/util/font_stack.hpp and src/mln/util/font_stack.cpp (the
// parts Resource::glyphs uses).

/// `FontStack`: an ordered list of font names.
pub type FontStack = Vec<String>;

/// `fontStackToString`: the names joined with ",".
#[must_use]
pub fn font_stack_to_string(font_stack: &FontStack) -> String {
    font_stack.join(",")
}
