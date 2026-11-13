// The C++ functions take `char16_t`: astral code points never reach them, so
// these take `u16`. A lone surrogate code unit is Cs.

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

/// Whether `codepoint` is in `first..=last`.
#[inline]
fn is_in_range(codepoint: u16, first: u16, last: u16) -> bool {
    codepoint >= first && codepoint <= last
}

// The following table comes from <http://www.unicode.org/Public/12.0.0/ucd/Blocks.txt>.
// Keep it synchronized with <http://www.unicode.org/Public/UCD/latest/ucd/Blocks.txt>.
macro_rules! define_is_in_unicode_block {
    ($($name:ident, $first:expr, $last:expr;)*) => {
        $(
            #[inline]
            fn $name(codepoint: u16) -> bool {
                ($first..=$last).contains(&codepoint)
            }
        )*
    };
}

define_is_in_unicode_block! {
    is_in_latin1_supplement, 0x0080, 0x00FF;
    is_in_arabic, 0x0600, 0x06FF;
    is_in_arabic_supplement, 0x0750, 0x077F;
    is_in_arabic_extended_a, 0x08A0, 0x08FF;
    is_in_hangul_jamo, 0x1100, 0x11FF;
    is_in_unified_canadian_aboriginal_syllabics, 0x1400, 0x167F;
    is_in_khmer, 0x1780, 0x17FF;
    is_in_unified_canadian_aboriginal_syllabics_extended, 0x18B0, 0x18FF;
    is_in_general_punctuation, 0x2000, 0x206F;
    is_in_letterlike_symbols, 0x2100, 0x214F;
    is_in_number_forms, 0x2150, 0x218F;
    is_in_miscellaneous_technical, 0x2300, 0x23FF;
    is_in_control_pictures, 0x2400, 0x243F;
    is_in_optical_character_recognition, 0x2440, 0x245F;
    is_in_enclosed_alphanumerics, 0x2460, 0x24FF;
    is_in_geometric_shapes, 0x25A0, 0x25FF;
    is_in_miscellaneous_symbols, 0x2600, 0x26FF;
    is_in_cjk_radicals_supplement, 0x2E80, 0x2EFF;
    is_in_kangxi_radicals, 0x2F00, 0x2FDF;
    is_in_ideographic_description_characters, 0x2FF0, 0x2FFF;
    is_in_cjk_symbols_and_punctuation, 0x3000, 0x303F;
    is_in_hiragana, 0x3040, 0x309F;
    is_in_katakana, 0x30A0, 0x30FF;
    is_in_bopomofo, 0x3100, 0x312F;
    is_in_hangul_compatibility_jamo, 0x3130, 0x318F;
    is_in_kanbun, 0x3190, 0x319F;
    is_in_bopomofo_extended, 0x31A0, 0x31BF;
    is_in_cjk_strokes, 0x31C0, 0x31EF;
    is_in_katakana_phonetic_extensions, 0x31F0, 0x31FF;
    is_in_enclosed_cjk_letters_and_months, 0x3200, 0x32FF;
    is_in_cjk_compatibility, 0x3300, 0x33FF;
    is_in_cjk_unified_ideographs_extension_a, 0x3400, 0x4DBF;
    is_in_yijing_hexagram_symbols, 0x4DC0, 0x4DFF;
    is_in_cjk_unified_ideographs, 0x4E00, 0x9FFF;
    is_in_yi_syllables, 0xA000, 0xA48F;
    is_in_yi_radicals, 0xA490, 0xA4CF;
    is_in_hangul_jamo_extended_a, 0xA960, 0xA97F;
    is_in_hangul_syllables, 0xAC00, 0xD7AF;
    is_in_hangul_jamo_extended_b, 0xD7B0, 0xD7FF;
    is_in_private_use_area, 0xE000, 0xF8FF;
    is_in_cjk_compatibility_ideographs, 0xF900, 0xFAFF;
    is_in_arabic_presentation_forms_a, 0xFB50, 0xFDFF;
    is_in_vertical_forms, 0xFE10, 0xFE1F;
    is_in_cjk_compatibility_forms, 0xFE30, 0xFE4F;
    is_in_small_form_variants, 0xFE50, 0xFE6F;
    is_in_arabic_presentation_forms_b, 0xFE70, 0xFEFF;
    is_in_halfwidth_and_fullwidth_forms, 0xFF00, 0xFFEF;
}

/// The vertical form of each punctuation character, sorted by key.
static VERTICAL_PUNCTUATION: [(u16, u16); 83] = [
    (0x0021, 0xFE15), // ! ︕
    (0x0023, 0xFF03), // # ＃
    (0x0024, 0xFF04), // $ ＄
    (0x0025, 0xFF05), // % ％
    (0x0026, 0xFF06), // & ＆
    (0x0028, 0xFE35), // ( ︵
    (0x0029, 0xFE36), // ) ︶
    (0x002A, 0xFF0A), // * ＊
    (0x002B, 0xFF0B), // + ＋
    (0x002C, 0xFE10), // , ︐
    (0x002D, 0xFE32), // - ︲
    (0x002E, 0x30FB), // . ・
    (0x002F, 0xFF0F), // / ／
    (0x003A, 0xFE13), // : ︓
    (0x003B, 0xFE14), // ; ︔
    (0x003C, 0xFE3F), // < ︿
    (0x003D, 0xFF1D), // = ＝
    (0x003E, 0xFE40), // > ﹀
    (0x003F, 0xFE16), // ? ︖
    (0x0040, 0xFF20), // @ ＠
    (0x005B, 0xFE47), // [ ﹇
    (0x005C, 0xFF3C), // \ ＼
    (0x005D, 0xFE48), // ] ﹈
    (0x005E, 0xFF3E), // ^ ＾
    (0x005F, 0xFE33), // _ ︳
    (0x0060, 0xFF40), // ` ｀
    (0x007B, 0xFE37), // { ︷
    (0x007C, 0x2015), // | ―
    (0x007D, 0xFE38), // } ︸
    (0x007E, 0xFF5E), // ~ ～
    (0x00A2, 0xFFE0), // ¢ ￠
    (0x00A3, 0xFFE1), // £ ￡
    (0x00A5, 0xFFE5), // ¥ ￥
    (0x00A6, 0xFFE4), // ¦ ￤
    (0x00AC, 0xFFE2), // ¬ ￢
    (0x00AF, 0xFFE3), // ¯ ￣
    (0x2013, 0xFE32), // – ︲
    (0x2014, 0xFE31), // — ︱
    (0x2018, 0xFE43), // ‘ ﹃
    (0x2019, 0xFE44), // ’ ﹄
    (0x201C, 0xFE41), // “ ﹁
    (0x201D, 0xFE42), // ” ﹂
    (0x2026, 0xFE19), // … ︙
    (0x2027, 0x30FB), // ‧ ・
    (0x20A9, 0xFFE6), // ₩ ￦
    (0x3001, 0xFE11), // 、 ︑
    (0x3002, 0xFE12), // 。 ︒
    (0x3008, 0xFE3F), // 〈 ︿
    (0x3009, 0xFE40), // 〉 ﹀
    (0x300A, 0xFE3D), // 《 ︽
    (0x300B, 0xFE3E), // 》 ︾
    (0x300C, 0xFE41), // 「 ﹁
    (0x300D, 0xFE42), // 」 ﹂
    (0x300E, 0xFE43), // 『 ﹃
    (0x300F, 0xFE44), // 』 ﹄
    (0x3010, 0xFE3B), // 【 ︻
    (0x3011, 0xFE3C), // 】 ︼
    (0x3014, 0xFE39), // 〔 ︹
    (0x3015, 0xFE3A), // 〕 ︺
    (0x3016, 0xFE17), // 〖 ︗
    (0x3017, 0xFE18), // 〗 ︘
    (0xFF01, 0xFE15), // ！ ︕
    (0xFF08, 0xFE35), // （ ︵
    (0xFF09, 0xFE36), // ） ︶
    (0xFF0C, 0xFE10), // ， ︐
    (0xFF0D, 0xFE32), // － ︲
    (0xFF0E, 0x30FB), // ． ・
    (0xFF1A, 0xFE13), // ： ︓
    (0xFF1B, 0xFE14), // ； ︔
    (0xFF1C, 0xFE3F), // ＜ ︿
    (0xFF1E, 0xFE40), // ＞ ﹀
    (0xFF1F, 0xFE16), // ？ ︖
    (0xFF3B, 0xFE47), // ［ ﹇
    (0xFF3D, 0xFE48), // ］ ﹈
    (0xFF3F, 0xFE33), // ＿ ︳
    (0xFF5B, 0xFE37), // ｛ ︷
    (0xFF5C, 0x2015), // ｜ ―
    (0xFF5D, 0xFE38), // ｝ ︸
    (0xFF5F, 0xFE35), // ｟ ︵
    (0xFF60, 0xFE36), // ｠ ︶
    (0xFF61, 0xFE12), // ｡ ︒
    (0xFF62, 0xFE41), // ｢ ﹁
    (0xFF63, 0xFE42), // ｣ ﹂
];

fn vertical_punctuation_get(chr: u16) -> Option<u16> {
    VERTICAL_PUNCTUATION.binary_search_by_key(&chr, |&(k, _)| k).ok().map(|i| VERTICAL_PUNCTUATION[i].1)
}

fn vertical_punctuation_contains(chr: u16) -> bool {
    vertical_punctuation_get(chr).is_some()
}

/// Returns whether a line break can be inserted after the character
/// indicated by the given Unicode codepoint due to word breaking.
#[must_use]
pub fn allows_word_breaking(chr: u16) -> bool {
    chr == 0x0a /* newline */
        || chr == 0x20 /* space */
        || chr == 0x26 /* ampersand */
        || chr == 0x29 /* close parenthesis */
        || chr == 0x2b /* plus sign */
        || chr == 0x2d /* hyphen-minus */
        || chr == 0x2f /* solidus */
        || chr == 0xad /* soft hyphen */
        || chr == 0xb7 /* middle dot */
        || chr == 0x200b /* zero-width space */
        || chr == 0x2010 /* hyphen */
        || chr == 0x2013 /* en dash */
}

/// Returns whether a line break can be inserted before the character
/// indicated by the given Unicode codepoint due to word breaking.
#[must_use]
pub fn allows_word_breaking_before(chr: u16) -> bool {
    chr == 0x28 /* open parenthesis */
}

/// Whether letter spacing applies to `chr` (it doesn't to Arabic).
#[must_use]
pub fn char_allows_letter_spacing(chr: u16) -> bool {
    !(is_in_arabic(chr)
        || is_in_arabic_supplement(chr)
        || is_in_arabic_extended_a(chr)
        || is_in_arabic_presentation_forms_a(chr)
        || is_in_arabic_presentation_forms_b(chr))
}

/// Returns whether the given string can be displayed with letter-spacing.
/// False for Arabic scripts, where letter-spacing will break ligatures.
#[must_use]
pub fn allows_letter_spacing(string: &[u16]) -> bool {
    string.iter().all(|&c| char_allows_letter_spacing(c))
}

/// Returns whether a line break can be inserted after the character
/// indicated by the given Unicode codepoint due to ideographic breaking.
#[must_use]
pub fn allows_ideographic_breaking(chr: u16) -> bool {
    // Allow U+2027 "Interpunct" for hyphenation of Chinese words
    if chr == 0x2027 {
        return true;
    }

    // Return early for characters outside all ideographic ranges.
    if chr < 0x2E80 {
        return false;
    }

    is_in_bopomofo(chr)
        || is_in_bopomofo_extended(chr)
        || is_in_cjk_compatibility(chr)
        || is_in_cjk_compatibility_forms(chr)
        || is_in_cjk_compatibility_ideographs(chr)
        || is_in_cjk_radicals_supplement(chr)
        || is_in_cjk_strokes(chr)
        || is_in_cjk_symbols_and_punctuation(chr)
        || is_in_cjk_unified_ideographs(chr)
        || is_in_cjk_unified_ideographs_extension_a(chr)
        || is_in_enclosed_cjk_letters_and_months(chr)
        || is_in_halfwidth_and_fullwidth_forms(chr)
        || is_in_hiragana(chr)
        || is_in_ideographic_description_characters(chr)
        || is_in_kangxi_radicals(chr)
        || is_in_katakana(chr)
        || is_in_katakana_phonetic_extensions(chr)
        || is_in_vertical_forms(chr)
        || is_in_yi_radicals(chr)
        || is_in_yi_syllables(chr)

    // The following blocks also allow ideographic breaking; however, for other
    // reasons, Mapbox GL lacks support for codepoints beyond U+FFFF.
    // https://github.com/mapbox/mapbox-gl/issues/29
}

/// Conservative set of characters expected to have relatively fixed sizes
/// and advances.
#[must_use]
pub fn allows_fixed_width_glyph_generation(chr: u16) -> bool {
    // Mirrors the BMP ranges in GL JS's codePointUsesLocalIdeographFontFamily.
    // Astral-plane ranges are not included because GlyphID only carries BMP
    // codepoints for local glyph generation.
    // Bopomofo tone marks.
    is_in_range(chr, 0x02EA, 0x02EB)
        // Hangul Jamo.
        || is_in_hangul_jamo(chr)
        // CJK Radicals Supplement, Kangxi Radicals.
        || is_in_cjk_radicals_supplement(chr)
        || is_in_kangxi_radicals(chr)
        // CJK Symbols and Punctuation, Hiragana, Katakana.
        || is_in_cjk_symbols_and_punctuation(chr)
        || is_in_hiragana(chr)
        || is_in_katakana(chr)
        // Bopomofo.
        || is_in_range(chr, 0x3105, 0x312F)
        // Hangul Compatibility Jamo.
        || is_in_range(chr, 0x3131, 0x318E)
        // Bopomofo Extended through CJK Unified Ideographs Extension A.
        || is_in_bopomofo_extended(chr)
        || is_in_cjk_strokes(chr)
        || is_in_katakana_phonetic_extensions(chr)
        || is_in_enclosed_cjk_letters_and_months(chr)
        || is_in_cjk_compatibility(chr)
        || is_in_cjk_unified_ideographs_extension_a(chr)
        // CJK Unified Ideographs and Yi Syllables.
        || is_in_cjk_unified_ideographs(chr)
        || is_in_range(chr, 0xA000, 0xA48C)
        // Yi Radicals.
        || is_in_range(chr, 0xA490, 0xA4C6)
        // Hangul Jamo Extended-A.
        || is_in_range(chr, 0xA960, 0xA97C)
        // Hangul Syllables and Hangul Jamo Extended-B.
        || is_in_hangul_syllables(chr)
        || is_in_range(chr, 0xD7B0, 0xD7C6)
        // Hangul Jamo Extended-B.
        || is_in_range(chr, 0xD7CB, 0xD7FB)
        // CJK Compatibility Ideographs.
        || is_in_range(chr, 0xF900, 0xFA6D)
        // CJK Compatibility Ideographs.
        || is_in_range(chr, 0xFA70, 0xFAD9)
        // Vertical Forms.
        || is_in_vertical_forms(chr)
        // CJK Compatibility Forms.
        || is_in_cjk_compatibility_forms(chr)
        // Halfwidth and Fullwidth Forms.
        || is_in_halfwidth_and_fullwidth_forms(chr)
}

/// Returns whether the whole 256-codepoint glyph range can be generated
/// locally. Use this for range-level decisions because some ranges are
/// mixed.
#[must_use]
pub fn glyph_range_is_entirely_locally_generated(range_start: u16) -> bool {
    // Some ranges (notably 0xFF00, where Halfwidth/Fullwidth Forms ends at
    // 0xFFEF and the Specials block continues to 0xFFFD) mix locally-rendered
    // and non-local codepoints, so it is not safe to infer the whole range
    // from the first codepoint alone.
    const GLYPHS_PER_RANGE: u32 = 256;
    for offset in 0..GLYPHS_PER_RANGE {
        // static_cast<char16_t>(rangeStart + offset): wraps past 0xFFFF.
        if !allows_fixed_width_glyph_generation((u32::from(range_start) + offset) as u16) {
            return false;
        }
    }
    true
}

/// Returns whether any substring of the given string can be drawn as
/// vertical text with upright glyphs.
#[must_use]
pub fn allows_vertical_writing_mode(string: &[u16]) -> bool {
    string.iter().any(|&c| has_upright_vertical_orientation(c))
}

// The following logic comes from
// <http://www.unicode.org/Public/12.0.0/ucd/VerticalOrientation.txt>.
// Keep it synchronized with
// <http://www.unicode.org/Public/UCD/latest/ucd/VerticalOrientation.txt>.
// The data file denotes with “U” or “Tu” any codepoint that may be drawn
// upright in vertical text but does not distinguish between upright and
// “neutral” characters.

/// Returns true if the given Unicode codepoint identifies a character with
/// upright orientation.
///
/// A character has upright orientation if it is drawn upright (unrotated)
/// whether the line is oriented horizontally or vertically, even if both
/// adjacent characters can be rotated. For example, a Chinese character is
/// always drawn upright. An uprightly oriented character causes an adjacent
/// “neutral” character to be drawn upright as well.
#[must_use]
pub fn has_upright_vertical_orientation(chr: u16) -> bool {
    if chr == 0x02EA /* ˪ */ || chr == 0x02EB
    /* ˫ */
    {
        return true;
    }

    // Return early for characters outside all ranges whose characters remain
    // upright in vertical writing mode.
    if chr < 0x1100 {
        return false;
    }

    if is_in_bopomofo(chr) || is_in_bopomofo_extended(chr) {
        return true;
    }
    if is_in_cjk_compatibility_forms(chr) && !(0xFE49..=0xFE4F).contains(&chr) {
        return true;
    }
    if is_in_cjk_compatibility(chr)
        || is_in_cjk_compatibility_ideographs(chr)
        || is_in_cjk_radicals_supplement(chr)
        || is_in_cjk_strokes(chr)
    {
        return true;
    }
    if is_in_cjk_symbols_and_punctuation(chr)
        && !(0x3008..=0x3011).contains(&chr)
        && !(0x3014..=0x301F).contains(&chr)
        && chr != 0x3030
    /* 〰 */
    {
        return true;
    }
    if is_in_cjk_unified_ideographs(chr)
        || is_in_cjk_unified_ideographs_extension_a(chr)
        || is_in_enclosed_cjk_letters_and_months(chr)
        || is_in_hangul_compatibility_jamo(chr)
        || is_in_hangul_jamo(chr)
        || is_in_hangul_jamo_extended_a(chr)
        || is_in_hangul_jamo_extended_b(chr)
        || is_in_hangul_syllables(chr)
        || is_in_hiragana(chr)
        || is_in_ideographic_description_characters(chr)
        || is_in_kanbun(chr)
        || is_in_kangxi_radicals(chr)
    {
        return true;
    }
    if is_in_katakana(chr) && chr != 0x30FC
    /* ー */
    {
        return true;
    }
    if is_in_katakana_phonetic_extensions(chr) {
        return true;
    }
    if is_in_halfwidth_and_fullwidth_forms(chr)
        && chr != 0xFF08 /* （ */
        && chr != 0xFF09 /* ） */
        && chr != 0xFF0D /* － */
        && !(0xFF1A..=0xFF1E).contains(&chr)
        && chr != 0xFF3B /* ［ */
        && chr != 0xFF3D /* ］ */
        && chr != 0xFF3F /* ＿ */
        && !(0xFF5B..=0xFFDF).contains(&chr)
        && chr != 0xFFE3 /* ￣ */
        && !(0xFFE8..=0xFFEF).contains(&chr)
    {
        return true;
    }
    if is_in_small_form_variants(chr) && !(0xFE58..=0xFE5E).contains(&chr) && !(0xFE63..=0xFE66).contains(&chr) {
        return true;
    }
    if is_in_unified_canadian_aboriginal_syllabics(chr)
        || is_in_unified_canadian_aboriginal_syllabics_extended(chr)
        || is_in_vertical_forms(chr)
        || is_in_yijing_hexagram_symbols(chr)
        || is_in_yi_syllables(chr)
        || is_in_yi_radicals(chr)
    {
        return true;
    }

    // https://github.com/mapbox/mapbox-gl/issues/29 (astral blocks not
    // supported).

    false
}

/// Returns true if the given Unicode codepoint identifies a character with
/// neutral orientation.
///
/// A character has neutral orientation if it may be drawn rotated or
/// unrotated when the line is oriented vertically, depending on the
/// orientation of the adjacent characters. For example, along a verticlly
/// oriented line, the vulgar fraction ½ is drawn upright among Chinese
/// characters but rotated among Latin letters. A neutrally oriented character
/// does not influence whether an adjacent character is drawn upright or
/// rotated.
#[must_use]
pub fn has_neutral_vertical_orientation(chr: u16) -> bool {
    if is_in_latin1_supplement(chr)
        && (chr == 0x00A7 /* § */
            || chr == 0x00A9 /* © */
            || chr == 0x00AE /* ® */
            || chr == 0x00B1 /* ± */
            || chr == 0x00BC /* ¼ */
            || chr == 0x00BD /* ½ */
            || chr == 0x00BE /* ¾ */
            || chr == 0x00D7 /* × */
            || chr == 0x00F7/* ÷ */)
    {
        return true;
    }
    if is_in_general_punctuation(chr)
        && (chr == 0x2016 /* ‖ */
            || chr == 0x2020 /* † */
            || chr == 0x2021 /* ‡ */
            || chr == 0x2030 /* ‰ */
            || chr == 0x2031 /* ‱ */
            || chr == 0x203B /* ※ */
            || chr == 0x203C /* ‼ */
            || chr == 0x2042 /* ⁂ */
            || chr == 0x2047 /* ⁇ */
            || chr == 0x2048 /* ⁈ */
            || chr == 0x2049 /* ⁉ */
            || chr == 0x2051/* ⁑ */)
    {
        return true;
    }
    if is_in_letterlike_symbols(chr) || is_in_number_forms(chr) {
        return true;
    }
    if is_in_miscellaneous_technical(chr)
        && ((0x2300..=0x2307).contains(&chr)
            || (0x230C..=0x231F).contains(&chr)
            || (0x2324..=0x2328).contains(&chr)
            || chr == 0x232B /* ⌫ */
            || (0x237D..=0x239A).contains(&chr)
            || (0x23BE..=0x23CD).contains(&chr)
            || chr == 0x23CF /* ⏏ */
            || (0x23D1..=0x23DB).contains(&chr)
            || (0x23E2..=0x23FF).contains(&chr))
    {
        return true;
    }
    if is_in_control_pictures(chr)
        || is_in_optical_character_recognition(chr)
        || is_in_enclosed_alphanumerics(chr)
        || is_in_geometric_shapes(chr)
    {
        return true;
    }
    // Quirk kept: these ranges (⬒..⬯ etc.) lie outside the Miscellaneous
    // Symbols block they are tested in, so this branch is never taken.
    if is_in_miscellaneous_symbols(chr)
        && ((0x2B12..=0x2B2F).contains(&chr) || (0x2B50..=0x2B59).contains(&chr) || (0x2BB8..=0x2BEB).contains(&chr))
    {
        return true;
    }
    if is_in_cjk_symbols_and_punctuation(chr)
        || is_in_katakana(chr)
        || is_in_private_use_area(chr)
        || is_in_cjk_compatibility_forms(chr)
        || is_in_small_form_variants(chr)
        || is_in_halfwidth_and_fullwidth_forms(chr)
    {
        return true;
    }
    if chr == 0x221E /* ∞ */
        || chr == 0x2234 /* ∴ */
        || chr == 0x2235 /* ∵ */
        || (0x2700..=0x2767).contains(&chr)
        || (0x2776..=0x2793).contains(&chr)
        || chr == 0xFFFC /* object replacement character */
        || chr == 0xFFFD
    /* replacement character */
    {
        return true;
    }
    false
}

/// Returns true if the given Unicode codepoint identifies a character with
/// rotated orientation.
///
/// A character has rotated orientation if it is drawn rotated when the line
/// is oriented vertically, even if both adjacent characters are upright. For
/// example, a Latin letter is drawn rotated along a vertical line. A rotated
/// character causes an adjacent “neutral” character to be drawn rotated as
/// well.
#[must_use]
pub fn has_rotated_vertical_orientation(chr: u16) -> bool {
    !(has_upright_vertical_orientation(chr) || has_neutral_vertical_orientation(chr))
}

/// Returns a copy of the given string with punctuation characters replaced
/// with their vertical forms wherever applicable.
///
/// Replaces "horizontal" with "vertical" punctuation in place. Does not
/// re-order or change length of string (`TaggedString::verticalizePunctuation`
/// depends on this behavior).
#[must_use]
pub fn verticalize_punctuation_str(input: &[u16]) -> Vec<u16> {
    let mut output = Vec::with_capacity(input.len());

    for i in 0..input.len() {
        let next_char_code = if i < input.len() - 1 { input[i + 1] } else { 0 };
        let prev_char_code = if i != 0 { input[i - 1] } else { 0 };

        // The C++ reads input[i + 1] / input[i - 1] only when the char code
        // is non-zero, so the indices are in range here too.
        let can_replace_punctuation = (next_char_code == 0
            || !has_rotated_vertical_orientation(next_char_code)
            || vertical_punctuation_contains(input[i + 1]))
            && (prev_char_code == 0
                || !has_rotated_vertical_orientation(prev_char_code)
                || vertical_punctuation_contains(input[i - 1]));

        let repl = if can_replace_punctuation { verticalize_punctuation(input[i]) } else { 0 };
        if repl != 0 {
            output.push(repl);
        } else {
            output.push(input[i]);
        }
    }

    output
}

/// Returns the form of the given character appropriate for vertical text:
/// the character’s specialized vertical form; 0 if not applicable.
#[must_use]
pub fn verticalize_punctuation(chr: u16) -> u16 {
    vertical_punctuation_get(chr).unwrap_or(0)
}

/// Whether `chr` is in a supported script: not one of the common scripts
/// that need complex shaping (Indic, Sinhala, Tibetan, Myanmar, Khmer).
#[must_use]
pub fn char_in_supported_script(chr: u16) -> bool {
    // This is a rough heuristic: whether we "can render" a script
    // actually depends on the properties of the font being used
    // and whether differences from the ideal rendering are considered
    // semantically significant.

    // These blocks cover common scripts that require
    // complex text shaping, based on unicode script metadata:
    // http://www.unicode.org/repos/cldr/trunk/common/properties/scriptMetadata.txt
    // where "Web Rank <= 32" "Shaping Required = YES"
    !((0x0900..=0x0DFF).contains(&chr)
        // Main blocks for Indic scripts and Sinhala
        || (0x0F00..=0x109F).contains(&chr)
        // Main blocks for Tibetan and Myanmar
        || is_in_khmer(chr))
}

/// Whether every UTF-16 code unit of `input` is in a supported script.
pub fn is_string_in_supported_script(input: &str) -> bool {
    input.encode_utf16().all(char_in_supported_script)
}

#[must_use]
pub fn is_char_in_complex_shaping_script(chr: u16) -> bool {
    is_in_arabic(chr)
        || is_in_arabic_supplement(chr)
        || is_in_arabic_extended_a(chr)
        || is_in_arabic_presentation_forms_a(chr)
        || is_in_arabic_presentation_forms_b(chr)
}

#[must_use]
pub fn is_whitespace(chr: u16) -> bool {
    chr == 0x20 || chr == 0x09 || chr == 0x0A || chr == 0x0B || chr == 0x0C || chr == 0x0D
}

/// The general category of a UTF-16 code unit; `None` for a surrogate.
fn general_category(chr: u16) -> Option<GeneralCategory> {
    char::from_u32(u32::from(chr)).map(UnicodeGeneralCategory::general_category)
}

#[must_use]
pub fn is_digit(chr: u16) -> bool {
    general_category(chr) == Some(GeneralCategory::DecimalNumber)
}

#[must_use]
pub fn is_uppercase(chr: u16) -> bool {
    general_category(chr) == Some(GeneralCategory::UppercaseLetter)
}

/// `(U_GET_GC_MASK(chr) & (U_GC_P_MASK | U_GC_S_MASK)) != 0`
/// (platform/default).
#[must_use]
pub fn is_punctuation_or_symbol(chr: u16) -> bool {
    char::from_u32(u32::from(chr)).is_some_and(|c| {
        matches!(c.general_category_group(), GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Symbol)
    })
}

#[must_use]
pub fn is_variation_selector1(chr: u16) -> bool {
    chr == 0xFE00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_punctuation_table_is_sorted() {
        assert!(VERTICAL_PUNCTUATION.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(verticalize_punctuation('(' as u16), 0xFE35);
        assert_eq!(verticalize_punctuation(0xFF63), 0xFE42);
        assert_eq!(verticalize_punctuation('a' as u16), 0);
    }

    #[test]
    fn general_category() {
        assert!(is_digit('7' as u16));
        assert!(is_digit(0x0663)); // ٣
        assert!(!is_digit('A' as u16));
        assert!(is_uppercase('A' as u16));
        assert!(is_uppercase(0x041C)); // М
        assert!(!is_uppercase('a' as u16));
        assert!(is_punctuation_or_symbol('-' as u16));
        assert!(is_punctuation_or_symbol('$' as u16));
        assert!(!is_punctuation_or_symbol('a' as u16));
        assert!(!is_punctuation_or_symbol(0xFFFF));
        assert!(!is_digit(0xD800) && !is_uppercase(0xD800) && !is_punctuation_or_symbol(0xD800));
    }
}
