//! The scripts that glyph PBFs can't shape, which the system's fonts draw.

use crate::text::glyph::FontFaceRange;

/// A script that needs a font of its own, since glyph PBFs can't shape it
/// (`i18n::char_in_supported_script`: the Indic scripts, Sinhala, Tibetan,
/// Myanmar and Khmer): the language that writes it, a letter of it, and its
/// Unicode blocks.
pub(crate) struct Script {
    /// The language to look its font up for.
    pub(crate) language: &'static str,
    pub(crate) probe: char,
    pub(crate) ranges: &'static [FontFaceRange],
}

pub(crate) const SCRIPTS: &[Script] = &[
    Script { language: "hi", probe: '\u{0915}', ranges: &[(0x0900, 0x097F), (0xA8E0, 0xA8FF)] },
    Script { language: "bn", probe: '\u{0995}', ranges: &[(0x0980, 0x09FF)] },
    Script { language: "pa", probe: '\u{0A15}', ranges: &[(0x0A00, 0x0A7F)] },
    Script { language: "gu", probe: '\u{0A95}', ranges: &[(0x0A80, 0x0AFF)] },
    Script { language: "or", probe: '\u{0B15}', ranges: &[(0x0B00, 0x0B7F)] },
    Script { language: "ta", probe: '\u{0B95}', ranges: &[(0x0B80, 0x0BFF)] },
    Script { language: "te", probe: '\u{0C15}', ranges: &[(0x0C00, 0x0C7F)] },
    Script { language: "kn", probe: '\u{0C95}', ranges: &[(0x0C80, 0x0CFF)] },
    Script { language: "ml", probe: '\u{0D15}', ranges: &[(0x0D00, 0x0D7F)] },
    Script { language: "si", probe: '\u{0D9A}', ranges: &[(0x0D80, 0x0DFF)] },
    Script { language: "bo", probe: '\u{0F40}', ranges: &[(0x0F00, 0x0FFF)] },
    Script { language: "my", probe: '\u{1000}', ranges: &[(0x1000, 0x109F), (0xA9E0, 0xA9FF), (0xAA60, 0xAA7F)] },
    Script { language: "km", probe: '\u{1780}', ranges: &[(0x1780, 0x17FF), (0x19E0, 0x19FF)] },
];
