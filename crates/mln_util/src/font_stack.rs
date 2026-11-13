/// An ordered list of font names.
pub type FontStack = Vec<String>;

/// Joins the names with ",".
#[must_use]
pub fn font_stack_to_string(font_stack: &FontStack) -> String {
    font_stack.join(",")
}
