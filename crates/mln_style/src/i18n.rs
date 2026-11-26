// The block tables cover the Basic Multilingual Plane, as Mapbox GL's do: a
// character beyond it is in none of them.

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

/// Whether `codepoint` is in `first..=last`.
#[inline]
fn is_in_range(chr: char, first: u32, last: u32) -> bool {
    (first..=last).contains(&u32::from(chr))
}

// The following table comes from <http://www.unicode.org/Public/12.0.0/ucd/Blocks.txt>.
// Keep it synchronized with <http://www.unicode.org/Public/UCD/latest/ucd/Blocks.txt>.
macro_rules! define_is_in_unicode_block {
    ($($name:ident, $first:expr, $last:expr;)*) => {
        $(
            #[inline]
            fn $name(chr: char) -> bool {
                ($first..=$last).contains(&u32::from(chr))
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
static VERTICAL_PUNCTUATION: [(char, char); 83] = [
    ('\u{0021}', '\u{FE15}'), // ! ︕
    ('\u{0023}', '\u{FF03}'), // # ＃
    ('\u{0024}', '\u{FF04}'), // $ ＄
    ('\u{0025}', '\u{FF05}'), // % ％
    ('\u{0026}', '\u{FF06}'), // & ＆
    ('\u{0028}', '\u{FE35}'), // ( ︵
    ('\u{0029}', '\u{FE36}'), // ) ︶
    ('\u{002A}', '\u{FF0A}'), // * ＊
    ('\u{002B}', '\u{FF0B}'), // + ＋
    ('\u{002C}', '\u{FE10}'), // , ︐
    ('\u{002D}', '\u{FE32}'), // - ︲
    ('\u{002E}', '\u{30FB}'), // . ・
    ('\u{002F}', '\u{FF0F}'), // / ／
    ('\u{003A}', '\u{FE13}'), // : ︓
    ('\u{003B}', '\u{FE14}'), // ; ︔
    ('\u{003C}', '\u{FE3F}'), // < ︿
    ('\u{003D}', '\u{FF1D}'), // = ＝
    ('\u{003E}', '\u{FE40}'), // > ﹀
    ('\u{003F}', '\u{FE16}'), // ? ︖
    ('\u{0040}', '\u{FF20}'), // @ ＠
    ('\u{005B}', '\u{FE47}'), // [ ﹇
    ('\u{005C}', '\u{FF3C}'), // \ ＼
    ('\u{005D}', '\u{FE48}'), // ] ﹈
    ('\u{005E}', '\u{FF3E}'), // ^ ＾
    ('\u{005F}', '\u{FE33}'), // _ ︳
    ('\u{0060}', '\u{FF40}'), // ` ｀
    ('\u{007B}', '\u{FE37}'), // { ︷
    ('\u{007C}', '\u{2015}'), // | ―
    ('\u{007D}', '\u{FE38}'), // } ︸
    ('\u{007E}', '\u{FF5E}'), // ~ ～
    ('\u{00A2}', '\u{FFE0}'), // ¢ ￠
    ('\u{00A3}', '\u{FFE1}'), // £ ￡
    ('\u{00A5}', '\u{FFE5}'), // ¥ ￥
    ('\u{00A6}', '\u{FFE4}'), // ¦ ￤
    ('\u{00AC}', '\u{FFE2}'), // ¬ ￢
    ('\u{00AF}', '\u{FFE3}'), // ¯ ￣
    ('\u{2013}', '\u{FE32}'), // – ︲
    ('\u{2014}', '\u{FE31}'), // — ︱
    ('\u{2018}', '\u{FE43}'), // ‘ ﹃
    ('\u{2019}', '\u{FE44}'), // ’ ﹄
    ('\u{201C}', '\u{FE41}'), // “ ﹁
    ('\u{201D}', '\u{FE42}'), // ” ﹂
    ('\u{2026}', '\u{FE19}'), // … ︙
    ('\u{2027}', '\u{30FB}'), // ‧ ・
    ('\u{20A9}', '\u{FFE6}'), // ₩ ￦
    ('\u{3001}', '\u{FE11}'), // 、 ︑
    ('\u{3002}', '\u{FE12}'), // 。 ︒
    ('\u{3008}', '\u{FE3F}'), // 〈 ︿
    ('\u{3009}', '\u{FE40}'), // 〉 ﹀
    ('\u{300A}', '\u{FE3D}'), // 《 ︽
    ('\u{300B}', '\u{FE3E}'), // 》 ︾
    ('\u{300C}', '\u{FE41}'), // 「 ﹁
    ('\u{300D}', '\u{FE42}'), // 」 ﹂
    ('\u{300E}', '\u{FE43}'), // 『 ﹃
    ('\u{300F}', '\u{FE44}'), // 』 ﹄
    ('\u{3010}', '\u{FE3B}'), // 【 ︻
    ('\u{3011}', '\u{FE3C}'), // 】 ︼
    ('\u{3014}', '\u{FE39}'), // 〔 ︹
    ('\u{3015}', '\u{FE3A}'), // 〕 ︺
    ('\u{3016}', '\u{FE17}'), // 〖 ︗
    ('\u{3017}', '\u{FE18}'), // 〗 ︘
    ('\u{FF01}', '\u{FE15}'), // ！ ︕
    ('\u{FF08}', '\u{FE35}'), // （ ︵
    ('\u{FF09}', '\u{FE36}'), // ） ︶
    ('\u{FF0C}', '\u{FE10}'), // ， ︐
    ('\u{FF0D}', '\u{FE32}'), // － ︲
    ('\u{FF0E}', '\u{30FB}'), // ． ・
    ('\u{FF1A}', '\u{FE13}'), // ： ︓
    ('\u{FF1B}', '\u{FE14}'), // ； ︔
    ('\u{FF1C}', '\u{FE3F}'), // ＜ ︿
    ('\u{FF1E}', '\u{FE40}'), // ＞ ﹀
    ('\u{FF1F}', '\u{FE16}'), // ？ ︖
    ('\u{FF3B}', '\u{FE47}'), // ［ ﹇
    ('\u{FF3D}', '\u{FE48}'), // ］ ﹈
    ('\u{FF3F}', '\u{FE33}'), // ＿ ︳
    ('\u{FF5B}', '\u{FE37}'), // ｛ ︷
    ('\u{FF5C}', '\u{2015}'), // ｜ ―
    ('\u{FF5D}', '\u{FE38}'), // ｝ ︸
    ('\u{FF5F}', '\u{FE35}'), // ｟ ︵
    ('\u{FF60}', '\u{FE36}'), // ｠ ︶
    ('\u{FF61}', '\u{FE12}'), // ｡ ︒
    ('\u{FF62}', '\u{FE41}'), // ｢ ﹁
    ('\u{FF63}', '\u{FE42}'), // ｣ ﹂
];

fn vertical_punctuation_get(chr: char) -> Option<char> {
    VERTICAL_PUNCTUATION.binary_search_by_key(&chr, |&(k, _)| k).ok().map(|i| VERTICAL_PUNCTUATION[i].1)
}

fn vertical_punctuation_contains(chr: char) -> bool {
    vertical_punctuation_get(chr).is_some()
}

/// Whether letter spacing applies to `chr` (it doesn't to Arabic).
#[must_use]
pub(crate) fn char_allows_letter_spacing(chr: char) -> bool {
    !(is_in_arabic(chr)
        || is_in_arabic_supplement(chr)
        || is_in_arabic_extended_a(chr)
        || is_in_arabic_presentation_forms_a(chr)
        || is_in_arabic_presentation_forms_b(chr))
}

/// Returns whether the given string can be displayed with letter-spacing.
/// False for Arabic scripts, where letter-spacing will break ligatures.
#[must_use]
pub fn allows_letter_spacing(string: &[char]) -> bool {
    string.iter().all(|&c| char_allows_letter_spacing(c))
}

/// Conservative set of characters expected to have relatively fixed sizes
/// and advances.
#[must_use]
pub fn allows_fixed_width_glyph_generation(chr: char) -> bool {
    // Mirrors the BMP ranges in GL JS's codePointUsesLocalIdeographFontFamily.
    // Astral-plane ranges are not included because glyph PBFs, and so the
    // glyphs generated in their place, only cover the BMP.
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

/// Returns whether the whole 256-codepoint glyph range that starts at
/// `range_start` can be generated locally. Use this for range-level decisions
/// because some ranges are mixed. Code points past U+FFFF wrap around, and
/// surrogates can't be generated.
#[must_use]
pub fn glyph_range_is_entirely_locally_generated(range_start: u16) -> bool {
    // Some ranges (notably 0xFF00, where Halfwidth/Fullwidth Forms ends at
    // 0xFFEF and the Specials block continues to 0xFFFD) mix locally-rendered
    // and non-local codepoints, so it is not safe to infer the whole range
    // from the first codepoint alone.
    const GLYPHS_PER_RANGE: u16 = 256;
    (0..GLYPHS_PER_RANGE).all(|offset| {
        char::from_u32(u32::from(range_start.wrapping_add(offset))).is_some_and(allows_fixed_width_glyph_generation)
    })
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
pub fn has_upright_vertical_orientation(chr: char) -> bool {
    if chr == '\u{02EA}' /* ˪ */ || chr == '\u{02EB}'
    /* ˫ */
    {
        return true;
    }

    // Return early for characters outside all ranges whose characters remain
    // upright in vertical writing mode.
    if chr < '\u{1100}' {
        return false;
    }

    if is_in_bopomofo(chr) || is_in_bopomofo_extended(chr) {
        return true;
    }
    if is_in_cjk_compatibility_forms(chr) && !('\u{FE49}'..='\u{FE4F}').contains(&chr) {
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
        && !('\u{3008}'..='\u{3011}').contains(&chr)
        && !('\u{3014}'..='\u{301F}').contains(&chr)
        && chr != '\u{3030}'
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
    if is_in_katakana(chr) && chr != '\u{30FC}'
    /* ー */
    {
        return true;
    }
    if is_in_katakana_phonetic_extensions(chr) {
        return true;
    }
    if is_in_halfwidth_and_fullwidth_forms(chr)
        && chr != '\u{FF08}' /* （ */
        && chr != '\u{FF09}' /* ） */
        && chr != '\u{FF0D}' /* － */
        && !('\u{FF1A}'..='\u{FF1E}').contains(&chr)
        && chr != '\u{FF3B}' /* ［ */
        && chr != '\u{FF3D}' /* ］ */
        && chr != '\u{FF3F}' /* ＿ */
        && !('\u{FF5B}'..='\u{FFDF}').contains(&chr)
        && chr != '\u{FFE3}' /* ￣ */
        && !('\u{FFE8}'..='\u{FFEF}').contains(&chr)
    {
        return true;
    }
    if is_in_small_form_variants(chr)
        && !('\u{FE58}'..='\u{FE5E}').contains(&chr)
        && !('\u{FE63}'..='\u{FE66}').contains(&chr)
    {
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
pub(crate) fn has_neutral_vertical_orientation(chr: char) -> bool {
    if is_in_latin1_supplement(chr)
        && (chr == '\u{00A7}' /* § */
            || chr == '\u{00A9}' /* © */
            || chr == '\u{00AE}' /* ® */
            || chr == '\u{00B1}' /* ± */
            || chr == '\u{00BC}' /* ¼ */
            || chr == '\u{00BD}' /* ½ */
            || chr == '\u{00BE}' /* ¾ */
            || chr == '\u{00D7}' /* × */
            || chr == '\u{00F7}'/* ÷ */)
    {
        return true;
    }
    if is_in_general_punctuation(chr)
        && (chr == '\u{2016}' /* ‖ */
            || chr == '\u{2020}' /* † */
            || chr == '\u{2021}' /* ‡ */
            || chr == '\u{2030}' /* ‰ */
            || chr == '\u{2031}' /* ‱ */
            || chr == '\u{203B}' /* ※ */
            || chr == '\u{203C}' /* ‼ */
            || chr == '\u{2042}' /* ⁂ */
            || chr == '\u{2047}' /* ⁇ */
            || chr == '\u{2048}' /* ⁈ */
            || chr == '\u{2049}' /* ⁉ */
            || chr == '\u{2051}'/* ⁑ */)
    {
        return true;
    }
    if is_in_letterlike_symbols(chr) || is_in_number_forms(chr) {
        return true;
    }
    if is_in_miscellaneous_technical(chr)
        && (('\u{2300}'..='\u{2307}').contains(&chr)
            || ('\u{230C}'..='\u{231F}').contains(&chr)
            || ('\u{2324}'..='\u{2328}').contains(&chr)
            || chr == '\u{232B}' /* ⌫ */
            || ('\u{237D}'..='\u{239A}').contains(&chr)
            || ('\u{23BE}'..='\u{23CD}').contains(&chr)
            || chr == '\u{23CF}' /* ⏏ */
            || ('\u{23D1}'..='\u{23DB}').contains(&chr)
            || ('\u{23E2}'..='\u{23FF}').contains(&chr))
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
        && (('\u{2B12}'..='\u{2B2F}').contains(&chr)
            || ('\u{2B50}'..='\u{2B59}').contains(&chr)
            || ('\u{2BB8}'..='\u{2BEB}').contains(&chr))
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
    if chr == '\u{221E}' /* ∞ */
        || chr == '\u{2234}' /* ∴ */
        || chr == '\u{2235}' /* ∵ */
        || ('\u{2700}'..='\u{2767}').contains(&chr)
        || ('\u{2776}'..='\u{2793}').contains(&chr)
        || chr == '\u{FFFC}' /* object replacement character */
        || chr == '\u{FFFD}'
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
pub(crate) fn has_rotated_vertical_orientation(chr: char) -> bool {
    !(has_upright_vertical_orientation(chr) || has_neutral_vertical_orientation(chr))
}

/// Returns a copy of the given string with punctuation characters replaced
/// with their vertical forms wherever applicable.
///
/// Replaces "horizontal" with "vertical" punctuation in place. Does not
/// re-order or change length of string (`TaggedString::verticalizePunctuation`
/// depends on this behavior). A NUL neighbor counts as no neighbor.
#[must_use]
pub fn verticalize_punctuation_str(input: &[char]) -> Vec<char> {
    // A neighbor allows the replacement unless it is rotated and has no
    // vertical form of its own.
    let allows = |neighbor: Option<&char>| {
        neighbor.is_none_or(|&c| c == '\0' || !has_rotated_vertical_orientation(c) || vertical_punctuation_contains(c))
    };
    (0..input.len())
        .map(|i| {
            let can_replace_punctuation = allows(input.get(i + 1)) && allows(i.checked_sub(1).map(|i| &input[i]));
            can_replace_punctuation.then(|| verticalize_punctuation(input[i])).flatten().unwrap_or(input[i])
        })
        .collect()
}

/// Returns the form of the given character appropriate for vertical text:
/// the character’s specialized vertical form, if it has one.
#[must_use]
pub fn verticalize_punctuation(chr: char) -> Option<char> {
    vertical_punctuation_get(chr)
}

/// Whether `chr` is in a supported script: not one of the common scripts
/// that need complex shaping (Indic, Sinhala, Tibetan, Myanmar, Khmer).
#[must_use]
pub fn char_in_supported_script(chr: char) -> bool {
    // This is a rough heuristic: whether we "can render" a script
    // actually depends on the properties of the font being used
    // and whether differences from the ideal rendering are considered
    // semantically significant.

    // These blocks cover common scripts that require
    // complex text shaping, based on unicode script metadata:
    // http://www.unicode.org/repos/cldr/trunk/common/properties/scriptMetadata.txt
    // where "Web Rank <= 32" "Shaping Required = YES"
    !(('\u{0900}'..='\u{0DFF}').contains(&chr)
        // Main blocks for Indic scripts and Sinhala
        || ('\u{0F00}'..='\u{109F}').contains(&chr)
        // Main blocks for Tibetan and Myanmar
        || is_in_khmer(chr))
}

/// Whether every character of `input` is in a supported script.
pub fn is_string_in_supported_script(input: &str) -> bool {
    input.chars().all(char_in_supported_script)
}

#[must_use]
pub fn is_char_in_complex_shaping_script(chr: char) -> bool {
    is_in_arabic(chr)
        || is_in_arabic_supplement(chr)
        || is_in_arabic_extended_a(chr)
        || is_in_arabic_presentation_forms_a(chr)
        || is_in_arabic_presentation_forms_b(chr)
}

#[must_use]
pub fn is_whitespace(chr: char) -> bool {
    chr == '\u{0020}'
        || chr == '\u{0009}'
        || chr == '\u{000A}'
        || chr == '\u{000B}'
        || chr == '\u{000C}'
        || chr == '\u{000D}'
}

#[must_use]
pub fn is_digit(chr: char) -> bool {
    chr.general_category() == GeneralCategory::DecimalNumber
}

#[must_use]
pub fn is_uppercase(chr: char) -> bool {
    chr.general_category() == GeneralCategory::UppercaseLetter
}

/// `(U_GET_GC_MASK(chr) & (U_GC_P_MASK | U_GC_S_MASK)) != 0`
/// (platform/default).
#[must_use]
pub fn is_punctuation_or_symbol(chr: char) -> bool {
    matches!(chr.general_category_group(), GeneralCategoryGroup::Punctuation | GeneralCategoryGroup::Symbol)
}

/// Whether `chr` has the Unicode `Variation_Selector` property, so it selects
/// a variant of the character before it.
#[must_use]
pub fn is_variation_selector(chr: char) -> bool {
    matches!(chr, '\u{180B}'..='\u{180D}' | '\u{180F}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// Whether `chr` is drawn on its own rather than in a run that the device's
/// fonts shape: a space, dash, slash or parenthesis that a line may break
/// beside, or CJK text, which may break between any two characters or stand
/// upright in vertical text. Such a character loses no shaping drawn one at a
/// time.
#[must_use]
pub fn breaks_or_stands_upright(chr: char) -> bool {
    matches!(
        chr,
        '\n' | ' ' | '&' | '(' | ')' | '+' | '-' | '/' | '\u{AD}' | '\u{B7}'
            | '\u{200B}' | '\u{2010}' | '\u{2013}' | '\u{2027}'
            // CJK radicals through CJK Unified Ideographs, Yi, CJK
            // Compatibility Ideographs, Vertical Forms, CJK Compatibility
            // Forms, and Halfwidth and Fullwidth Forms.
            | '\u{2E80}'..='\u{A4CF}' | '\u{F900}'..='\u{FAFF}' | '\u{FE10}'..='\u{FE1F}'
            | '\u{FE30}'..='\u{FE4F}' | '\u{FF00}'..='\u{FFEF}'
    ) || has_upright_vertical_orientation(chr)
}

/// Whether `chr` belongs with the character before it, so that the font
/// that draws that one draws it too: a mark, or a joiner or non-joiner.
#[must_use]
pub fn extends_previous(chr: char) -> bool {
    chr.general_category_group() == GeneralCategoryGroup::Mark || matches!(chr, '\u{200C}' | '\u{200D}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_punctuation_table_is_sorted() {
        assert!(VERTICAL_PUNCTUATION.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(verticalize_punctuation('('), Some('\u{FE35}'));
        assert_eq!(verticalize_punctuation('\u{FF63}'), Some('\u{FE42}'));
        assert_eq!(verticalize_punctuation('a'), None);
    }

    #[test]
    fn general_category() {
        assert!(is_digit('7'));
        assert!(is_digit('٣'));
        assert!(!is_digit('A'));
        assert!(is_uppercase('A'));
        assert!(is_uppercase('М'));
        assert!(!is_uppercase('a'));
        assert!(is_punctuation_or_symbol('-'));
        assert!(is_punctuation_or_symbol('$'));
        assert!(!is_punctuation_or_symbol('a'));
        assert!(!is_punctuation_or_symbol('\u{FFFF}'));
        // Characters beyond the BMP have categories too.
        assert!(is_digit('\u{1D7CE}')); // MATHEMATICAL BOLD DIGIT ZERO
        assert!(is_uppercase('\u{10400}')); // DESERET CAPITAL LETTER LONG I
        assert!(is_punctuation_or_symbol('\u{1F600}'));
    }

    #[test]
    fn verticalizes_punctuation_between_upright_neighbors() {
        let s = |s: &str| s.chars().collect::<Vec<char>>();
        assert_eq!(verticalize_punctuation_str(&s("中(中)")), s("中\u{FE35}中\u{FE36}"));
        // A rotated neighbor keeps it horizontal.
        assert_eq!(verticalize_punctuation_str(&s("a(b")), s("a(b"));
        assert_eq!(verticalize_punctuation_str(&s("(")), s("\u{FE35}"));
    }
}
