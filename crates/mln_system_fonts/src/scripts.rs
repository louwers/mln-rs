//! The scripts that the map falls back to system fonts for.

use mln_text::glyph::FontFaceRange;

/// A script that needs a font of its own: the language that writes it, the
/// families that draw it on Linux, macOS and Windows in order of preference,
/// the words in the names of other families that do, a letter of it, and its
/// Unicode blocks.
///
/// The scripts are those that glyph PBFs can't shape
/// (`i18n::char_in_supported_script`: the Indic scripts, Sinhala, Tibetan,
/// Myanmar and Khmer), and CJK, whose ideographs, kana and hangul would
/// otherwise come from a glyph PBF for every few characters.
pub(crate) struct Script {
    /// The language to look its font up for, or `None` for the user's own
    /// languages: ideographs, which Chinese, Japanese and Korean draw
    /// differently.
    pub(crate) language: Option<&'static str>,
    /// Whether it needs complex shaping, which glyph PBFs can't do.
    pub(crate) needs_shaping: bool,
    /// For the scan of font files (`scan.rs`), where the platform has no
    /// font matcher.
    pub(crate) families: &'static [&'static str],
    pub(crate) keywords: &'static [&'static str],
    pub(crate) probe: char,
    pub(crate) ranges: &'static [FontFaceRange],
}

pub(crate) const SCRIPTS: &[Script] = &[
    Script {
        language: Some("hi"),
        needs_shaping: true,
        families: &["Noto Sans Devanagari", "Kohinoor Devanagari", "Devanagari Sangam MN", "Nirmala UI", "Mangal"],
        keywords: &["Devanagari"],
        probe: '\u{0915}',
        ranges: &[(0x0900, 0x097F), (0xA8E0, 0xA8FF)],
    },
    Script {
        language: Some("bn"),
        needs_shaping: true,
        families: &["Noto Sans Bengali", "Kohinoor Bangla", "Bangla Sangam MN", "Nirmala UI", "Vrinda"],
        keywords: &["Bengali", "Bangla"],
        probe: '\u{0995}',
        ranges: &[(0x0980, 0x09FF)],
    },
    Script {
        language: Some("pa"),
        needs_shaping: true,
        families: &["Noto Sans Gurmukhi", "Gurmukhi Sangam MN", "Gurmukhi MN", "Nirmala UI", "Raavi"],
        keywords: &["Gurmukhi"],
        probe: '\u{0A15}',
        ranges: &[(0x0A00, 0x0A7F)],
    },
    Script {
        language: Some("gu"),
        needs_shaping: true,
        families: &["Noto Sans Gujarati", "Kohinoor Gujarati", "Gujarati Sangam MN", "Nirmala UI", "Shruti"],
        keywords: &["Gujarati"],
        probe: '\u{0A95}',
        ranges: &[(0x0A80, 0x0AFF)],
    },
    Script {
        language: Some("or"),
        needs_shaping: true,
        families: &["Noto Sans Oriya", "Oriya Sangam MN", "Nirmala UI", "Kalinga"],
        keywords: &["Oriya", "Odia"],
        probe: '\u{0B15}',
        ranges: &[(0x0B00, 0x0B7F)],
    },
    Script {
        language: Some("ta"),
        needs_shaping: true,
        families: &["Noto Sans Tamil", "Tamil Sangam MN", "Nirmala UI", "Latha"],
        keywords: &["Tamil"],
        probe: '\u{0B95}',
        ranges: &[(0x0B80, 0x0BFF)],
    },
    Script {
        language: Some("te"),
        needs_shaping: true,
        families: &["Noto Sans Telugu", "Kohinoor Telugu", "Telugu Sangam MN", "Nirmala UI", "Gautami"],
        keywords: &["Telugu"],
        probe: '\u{0C15}',
        ranges: &[(0x0C00, 0x0C7F)],
    },
    Script {
        language: Some("kn"),
        needs_shaping: true,
        families: &["Noto Sans Kannada", "Kannada Sangam MN", "Nirmala UI", "Tunga"],
        keywords: &["Kannada"],
        probe: '\u{0C95}',
        ranges: &[(0x0C80, 0x0CFF)],
    },
    Script {
        language: Some("ml"),
        needs_shaping: true,
        families: &["Noto Sans Malayalam", "Malayalam Sangam MN", "Nirmala UI", "Kartika"],
        keywords: &["Malayalam"],
        probe: '\u{0D15}',
        ranges: &[(0x0D00, 0x0D7F)],
    },
    Script {
        language: Some("si"),
        needs_shaping: true,
        families: &["Noto Sans Sinhala", "Sinhala Sangam MN", "Nirmala UI", "Iskoola Pota"],
        keywords: &["Sinhala"],
        probe: '\u{0D9A}',
        ranges: &[(0x0D80, 0x0DFF)],
    },
    Script {
        language: Some("bo"),
        needs_shaping: true,
        families: &["Noto Serif Tibetan", "Kailasa", "Microsoft Himalaya"],
        keywords: &["Tibetan"],
        probe: '\u{0F40}',
        ranges: &[(0x0F00, 0x0FFF)],
    },
    Script {
        language: Some("my"),
        needs_shaping: true,
        families: &["Noto Sans Myanmar", "Myanmar Sangam MN", "Myanmar MN", "Myanmar Text", "Padauk"],
        keywords: &["Myanmar"],
        probe: '\u{1000}',
        ranges: &[(0x1000, 0x109F), (0xA9E0, 0xA9FF), (0xAA60, 0xAA7F)],
    },
    Script {
        language: Some("km"),
        needs_shaping: true,
        families: &["Noto Sans Khmer", "Khmer Sangam MN", "Khmer MN", "Khmer UI", "Leelawadee UI"],
        keywords: &["Khmer"],
        probe: '\u{1780}',
        ranges: &[(0x1780, 0x17FF), (0x19E0, 0x19FF)],
    },
    // Ideographs, kana, bopomofo and CJK punctuation and forms. A
    // collection's first face is often its Japanese one, which has the
    // ideographs of the others.
    Script {
        language: None,
        needs_shaping: false,
        families: &[
            "Noto Sans CJK JP",
            "Noto Sans CJK SC",
            "Noto Sans CJK TC",
            "Source Han Sans",
            "Hiragino Sans",
            "PingFang SC",
            "Yu Gothic",
            "Microsoft YaHei",
            "Droid Sans Fallback",
        ],
        keywords: &["CJK"],
        probe: '\u{4E2D}',
        ranges: &[
            (0x2E80, 0x2FFF),
            (0x3000, 0x312F),
            (0x3190, 0x33FF),
            (0x3400, 0x4DBF),
            (0x4E00, 0x9FFF),
            (0xF900, 0xFAFF),
            (0xFE10, 0xFE1F),
            (0xFE30, 0xFE4F),
            (0xFF00, 0xFFEF),
        ],
    },
    Script {
        language: Some("ko"),
        needs_shaping: false,
        families: &[
            "Noto Sans CJK KR",
            "Noto Sans KR",
            "Noto Sans CJK JP",
            "Source Han Sans K",
            "Apple SD Gothic Neo",
            "Malgun Gothic",
            "NanumGothic",
        ],
        keywords: &["CJK", "Hangul"],
        probe: '\u{D55C}',
        ranges: &[(0x1100, 0x11FF), (0x3130, 0x318F), (0xA960, 0xA97F), (0xAC00, 0xD7FF)],
    },
];
