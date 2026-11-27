//! The weight the map draws a variable font at.

use skrifa::Font;

/// The weight the map shapes and draws a variable font at: regular, within
/// the range of the font's `wght` axis. Every label that a font draws comes
/// out at this one weight, so a font whose default instance is far from
/// regular (the variable Noto Sans CJK defaults to Thin, 100) still draws as
/// body text. A device font loads at its face's `weight` instead
/// ([`FontFace::device`](crate::text::glyph::FontFace::device)).
pub(crate) const REGULAR_WEIGHT: f32 = 400.0;

/// `font` at `weight`, clamped to its `wght` axis: `font` itself when it has
/// no such axis or that is its default.
pub(crate) fn at_weight(font: Font, weight: f32) -> Font {
    let Some(axis) = font.axes().get_by_tag(skrifa::Tag::new(b"wght")) else {
        return font;
    };
    let weight = weight.clamp(axis.min_value(), axis.max_value());
    if weight == axis.default_value() {
        return font;
    }
    font.instance_builder().variations([(skrifa::Tag::new(b"wght"), weight)]).build()
}
