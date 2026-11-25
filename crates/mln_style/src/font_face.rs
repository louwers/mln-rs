//! The font faces of a style's `font-faces`, as the style describes them.
//! A map's text engine turns them into faces it can draw with.

/// The first and last code points of a range that a font face draws.
pub type FontFaceRange = (u32, u32);

/// A font file that draws a `text-font` name's characters in `ranges`, from
/// a style's `font-faces` or an app's fallback faces. A fallback face has no
/// name: every font stack uses it after the faces that name one of its fonts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFace {
    /// The `text-font` name it draws, or "" for a fallback.
    pub name: String,
    pub url: String,
    /// The code points it draws.
    pub ranges: Vec<FontFaceRange>,
}

pub type FontFaces = Vec<FontFace>;

/// The characters that fonts draw, which `is-supported-script` accepts in
/// addition to the scripts that need no complex shaping.
pub trait FontCoverage {
    /// Whether a font draws `c`.
    fn covers(&self, c: char) -> bool;
}

impl FontCoverage for FontFace {
    fn covers(&self, c: char) -> bool {
        ranges_cover(&self.ranges, c)
    }
}

/// A list of faces covers what any of its faces does.
impl<F: FontCoverage> FontCoverage for Vec<F> {
    fn covers(&self, c: char) -> bool {
        self.iter().any(|face| face.covers(c))
    }
}

/// Whether one of `ranges` holds `c`.
#[must_use]
pub fn ranges_cover(ranges: &[FontFaceRange], c: char) -> bool {
    let c = u32::from(c);
    ranges.iter().any(|&(first, last)| first <= c && c <= last)
}
