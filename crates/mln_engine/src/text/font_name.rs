//! What a `text-font` name says about the font it names, such as "Open Sans
//! Semibold Italic": a family, a weight and a style. Local fonts are looked
//! up by these, as MapLibre GL JS reads them for the glyphs it draws itself.

/// The parts of a font name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontName<'a> {
    /// The name without the style words at its end: "Open Sans". The whole
    /// name when it has nothing else.
    pub family: &'a str,
    /// The OpenType weight (`usWeightClass`) of the last weight word in the
    /// name, or 400.
    pub weight: u16,
    /// Whether the name says "Italic" or "Oblique".
    pub italic: bool,
}

/// The weight words, as whole words of a name in any case, with the weights
/// they name. A later word wins over an earlier one, so "Extra Bold" is 800
/// rather than 700.
const WEIGHTS: [(&str, u16); 17] = [
    ("thin", 100),
    ("hairline", 100),
    ("extra light", 200),
    ("ultra light", 200),
    ("light", 300),
    ("normal", 400),
    ("regular", 400),
    ("medium", 500),
    ("semibold", 600),
    ("demibold", 600),
    ("bold", 700),
    ("extra bold", 800),
    ("ultra bold", 800),
    ("black", 900),
    ("heavy", 900),
    ("extra black", 950),
    ("ultra black", 950),
];

/// The words that end a name without naming its family.
const STYLE_WORDS: [&str; 22] = [
    "thin",
    "hairline",
    "extra",
    "ultra",
    "semi",
    "demi",
    "extralight",
    "ultralight",
    "light",
    "normal",
    "regular",
    "book",
    "medium",
    "semibold",
    "demibold",
    "bold",
    "extrabold",
    "ultrabold",
    "black",
    "heavy",
    "italic",
    "oblique",
];

/// Reads `name`.
#[must_use]
pub fn parse(name: &str) -> FontName<'_> {
    let lower = name.to_ascii_lowercase();
    let weight = WEIGHTS.iter().rev().find(|(word, _)| has_word(&lower, word)).map_or(400, |&(_, weight)| weight);
    let italic = has_word(&lower, "italic") || has_word(&lower, "oblique");
    let mut family = name.trim_end();
    while let Some((rest, last)) = family.rsplit_once(' ') {
        if !STYLE_WORDS.iter().any(|word| last.eq_ignore_ascii_case(word)) {
            break;
        }
        family = rest.trim_end();
    }
    if STYLE_WORDS.iter().any(|word| family.eq_ignore_ascii_case(word)) {
        family = name.trim();
    }
    FontName { family, weight, italic }
}

/// Whether `word` is in `text` between word boundaries (`\b` in a regular
/// expression).
fn has_word(text: &str, word: &str) -> bool {
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    text.match_indices(word)
        .any(|(at, _)| !is_word(text[..at].chars().next_back()) && !is_word(text[at + word.len()..].chars().next()))
}

#[cfg(test)]
mod tests {
    use super::{FontName, parse};

    fn name(family: &str, weight: u16, italic: bool) -> FontName<'_> {
        FontName { family, weight, italic }
    }

    #[test]
    fn a_name_is_a_family_and_a_style() {
        assert_eq!(parse("Open Sans Regular"), name("Open Sans", 400, false));
        assert_eq!(parse("Open Sans Semibold Italic"), name("Open Sans", 600, true));
        assert_eq!(parse("Noto Sans CJK JP Bold"), name("Noto Sans CJK JP", 700, false));
        assert_eq!(parse("Roboto Extra Bold"), name("Roboto", 800, false));
        assert_eq!(parse("Arial Unicode MS Regular"), name("Arial Unicode MS", 400, false));
        assert_eq!(parse("Metropolis Black Oblique"), name("Metropolis", 900, true));
    }

    #[test]
    fn a_name_without_a_style_is_its_family() {
        assert_eq!(parse("Helvetica"), name("Helvetica", 400, false));
        assert_eq!(parse("Bold"), name("Bold", 700, false));
    }

    #[test]
    fn style_words_inside_a_family_stay() {
        assert_eq!(parse("Boldonse Regular"), name("Boldonse", 400, false));
        assert_eq!(parse("Light Sans Bold"), name("Light Sans", 700, false));
    }
}
