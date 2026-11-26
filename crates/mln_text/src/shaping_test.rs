use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::i18n;
use mln_style::image::{ImageContent, Impl, TextFit};
use mln_style::size::Size;
use mln_style::types::{IconTextFitType, SymbolAnchorType, TextJustifyType, TextWritingModeType, VerticalAlign};
use mln_util::constants::ONE_EM;
use mln_util::font_stack::FontStack;

use crate::atlas::{ImagePosition, ImagePositions, RectU16};
use crate::glyph::{
    Bounds, GlyphID, GlyphIDType, GlyphMetrics, GlyphPosition, GlyphPositions, PositionedGlyph, PositionedLine,
    Shaping, face_type,
};
use crate::shaper::{GlyphAdjust, ShapedText};
use crate::shaping::{PositionedIcon, ShapingOptions, shape_text};
use crate::tagged_string::{FontStackHash, SectionOptions, TaggedString, font_stack_hash};

fn u(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn add_stub_glyph(glyph_positions: &mut GlyphPositions, font_stack_hash: FontStackHash, glyph_id: GlyphID) {
    let metrics = GlyphMetrics { width: 10, height: 18, left: 1.0, top: -8.0, advance: 12, ..Default::default() };
    let position = GlyphPosition { metrics, ..Default::default() };
    glyph_positions.entry(font_stack_hash).or_default().insert(glyph_id, Some(position));
}

fn add_stub_glyphs(string: &TaggedString, glyph_positions: &mut GlyphPositions) {
    for i in 0..string.len() {
        let section = string.section_of(i);
        if section.image_id.is_some() {
            continue;
        }
        let glyphs = match &section.shaped {
            Some(shaped) => shaped.glyphs.clone(),
            None => std::iter::once(string.text()[i])
                .chain(
                    i18n::verticalize_punctuation(string.text()[i]).filter(|_| section.type_ == GlyphIDType::FontPBF),
                )
                .map(|c| GlyphID::from_char(c).unwrap())
                .collect(),
        };
        for glyph in glyphs {
            add_stub_glyph(glyph_positions, section.font_stack_hash, glyph);
        }
    }
}

fn shape_tagged_text(
    string: &TaggedString,
    writing_mode: TextWritingModeType,
    allow_vertical_placement: bool,
    image_positions: &ImagePositions,
) -> Shaping {
    let mut glyph_positions = GlyphPositions::new();
    add_stub_glyphs(string, &mut glyph_positions);

    shape_text(
        string,
        &ShapingOptions {
            max_width: 4096.0,
            line_height: ONE_EM,
            anchor: SymbolAnchorType::TopLeft,
            justify: TextJustifyType::Left,
            spacing: 0.0,
            offset: [0.0, 0.0],
            writing_mode,
            allow_vertical_placement,
            text_size: ONE_EM,
            text_size_at_bucket_zoom: ONE_EM,
        },
        &glyph_positions,
        image_positions,
    )
}

fn shape_line_label_with(text: &str, writing_mode: TextWritingModeType, allow_vertical_placement: bool) -> Shaping {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    shape_tagged_text(
        &TaggedString::new(u(text), SectionOptions::new(1.0, font_stack, GlyphIDType::FontPBF, None)),
        writing_mode,
        allow_vertical_placement,
        &ImagePositions::default(),
    )
}

fn shape_line_label(text: &str) -> Shaping {
    shape_line_label_with(text, TextWritingModeType::Vertical, false)
}

type GlyphOrientation = (u16, &'static str);

fn get_glyph_orientations(shaping: &Shaping) -> Vec<GlyphOrientation> {
    let mut result = Vec::new();
    for line in &shaping.lines {
        for glyph in &line.glyphs {
            result.push((glyph.glyph.code, if glyph.vertical { "upright" } else { "along-line" }));
        }
    }
    result
}

/// The expected orientations: (character, orientation).
fn expect(pairs: &[(char, &'static str)]) -> Vec<GlyphOrientation> {
    pairs.iter().map(|&(c, o)| (c as u32 as u16, o)).collect()
}

// gtest: Shaping.ZWSP
#[test]
fn zwsp() {
    let metrics = GlyphMetrics { width: 18, height: 18, left: 2.0, top: -8.0, advance: 21, ..Default::default() };
    let glyph_position = GlyphPosition { metrics, ..Default::default() };

    let font_stack: FontStack = vec!["font-stack".to_string()];
    let section_options = SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None);
    let layout_text_size = 16.0f32;
    let layout_text_size_at_bucket_zoom_level = 16.0f32;
    let mut glyph_positions = GlyphPositions::new();
    glyph_positions
        .entry(font_stack_hash(&font_stack))
        .or_default()
        .insert(GlyphID::from_char('中').unwrap(), Some(glyph_position));
    let image_positions = ImagePositions::default();

    let test_get_shaping = |string: &TaggedString, max_width_in_chars: u32| {
        shape_text(
            string,
            &ShapingOptions {
                max_width: max_width_in_chars as f32 * ONE_EM,
                line_height: ONE_EM,
                anchor: SymbolAnchorType::Center,
                justify: TextJustifyType::Center,
                spacing: 0.0,
                offset: [0.0, 0.0],
                writing_mode: TextWritingModeType::Horizontal,
                allow_vertical_placement: false,
                text_size: layout_text_size,
                text_size_at_bucket_zoom: layout_text_size_at_bucket_zoom_level,
            },
            &glyph_positions,
            &image_positions,
        )
    };

    // 3 lines
    // 中中中中中中
    // 中中中中中中
    // 中中
    {
        let string = TaggedString::new(
            u("中中\u{200b}中中\u{200b}中中\u{200b}中中中中中中\u{200b}中中"),
            section_options.clone(),
        );
        let shaping = test_get_shaping(&string, 5);
        assert_eq!(shaping.lines.len(), 3);
        assert_eq!(shaping.bounds.top, -36.0);
        assert_eq!(shaping.bounds.bottom, 36.0);
        assert_eq!(shaping.bounds.left, -63.0);
        assert_eq!(shaping.bounds.right, 63.0);
    }

    // 2 lines
    // 中中
    // 中
    {
        let string = TaggedString::new(u("中中\u{200b}中"), section_options.clone());
        let shaping = test_get_shaping(&string, 1);
        assert_eq!(shaping.lines.len(), 2);
        assert_eq!(shaping.bounds.top, -24.0);
        assert_eq!(shaping.bounds.bottom, 24.0);
        assert_eq!(shaping.bounds.left, -21.0);
        assert_eq!(shaping.bounds.right, 21.0);
    }

    // 1 line
    // 中中
    {
        let string = TaggedString::new(u("中中\u{200b}"), section_options.clone());
        let shaping = test_get_shaping(&string, 2);
        assert_eq!(shaping.lines.len(), 1);
        assert_eq!(shaping.bounds.top, -12.0);
        assert_eq!(shaping.bounds.bottom, 12.0);
        assert_eq!(shaping.bounds.left, -21.0);
        assert_eq!(shaping.bounds.right, 21.0);
    }

    // 1 empty line: a line never starts at a zero width space.
    {
        let string = TaggedString::new(u("\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}"), section_options);
        let shaping = test_get_shaping(&string, 1);
        assert_eq!(shaping.lines.len(), 1);
        assert_eq!(shaping.bounds.top, -12.0);
        assert_eq!(shaping.bounds.bottom, 12.0);
        assert_eq!(shaping.bounds.left, 0.0);
        assert_eq!(shaping.bounds.right, 0.0);
    }
}

// Regression cases ported from maplibre-gl-js#8205 for maplibre-native#4565.
// gtest: Shaping.VerticalDigitsBetweenCJKCharacters
#[test]
fn vertical_digits_between_cjk_characters() {
    let orientations = get_glyph_orientations(&shape_line_label("반포대로21길"));
    let expected = expect(&[
        ('반', "upright"),
        ('포', "upright"),
        ('대', "upright"),
        ('로', "upright"),
        ('2', "upright"),
        ('1', "upright"),
        ('길', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalTrailingDigits
#[test]
fn vertical_trailing_digits() {
    let orientations = get_glyph_orientations(&shape_line_label("身什戰33"));
    let expected =
        expect(&[('身', "upright"), ('什', "upright"), ('戰', "upright"), ('3', "upright"), ('3', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalDigitsOfAnyLength
#[test]
fn vertical_digits_of_any_length() {
    let orientations = get_glyph_orientations(&shape_line_label("国道1234号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('1', "upright"),
        ('2', "upright"),
        ('3', "upright"),
        ('4', "upright"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalWhitespaceSeparatedDigit
#[test]
fn vertical_whitespace_separated_digit() {
    let orientations = get_glyph_orientations(&shape_line_label("身什戰 1"));
    let expected =
        expect(&[('身', "upright"), ('什', "upright"), ('戰', "upright"), (' ', "along-line"), ('1', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalSingleUppercaseLetter
#[test]
fn vertical_single_uppercase_letter() {
    let orientations = get_glyph_orientations(&shape_line_label("国道A号"));
    let expected = expect(&[('国', "upright"), ('道', "upright"), ('A', "upright"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalDecomposedLatinLetter
#[test]
fn vertical_decomposed_latin_letter() {
    let orientations = get_glyph_orientations(&shape_line_label("国道e\u{0301}号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('e', "along-line"),
        ('\u{0301}', "along-line"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalLowercaseGreekLetter
#[test]
fn vertical_lowercase_greek_letter() {
    let orientations = get_glyph_orientations(&shape_line_label("国道α号"));
    let expected = expect(&[('国', "upright"), ('道', "upright"), ('α', "along-line"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalNonLatinUppercaseCode
#[test]
fn vertical_non_latin_uppercase_code() {
    let orientations = get_glyph_orientations(&shape_line_label("国道\u{041C}1号"));
    let expected =
        expect(&[('国', "upright"), ('道', "upright"), ('\u{041C}', "upright"), ('1', "upright"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalComplexScript
#[test]
fn vertical_complex_script() {
    let orientations = get_glyph_orientations(&shape_line_label("国道ب号"));
    let expected = expect(&[('国', "upright"), ('道', "upright"), ('ب', "along-line"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalProlongedSoundMark
#[test]
fn vertical_prolonged_sound_mark() {
    let orientations = get_glyph_orientations(&shape_line_label("札幌タワー"));
    let expected =
        expect(&[('札', "upright"), ('幌', "upright"), ('タ', "upright"), ('ワ', "upright"), ('ー', "along-line")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalWaveDashBetweenDigits
#[test]
fn vertical_wave_dash_between_digits() {
    let orientations = get_glyph_orientations(&shape_line_label("身什戰1〜2"));
    let expected = expect(&[
        ('身', "upright"),
        ('什', "upright"),
        ('戰', "upright"),
        ('1', "upright"),
        ('〜', "along-line"),
        ('2', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalLowercaseLatinWord
#[test]
fn vertical_lowercase_latin_word() {
    let orientations = get_glyph_orientations(&shape_line_label("two 身什戰"));
    let expected = expect(&[
        ('t', "along-line"),
        ('w', "along-line"),
        ('o', "along-line"),
        (' ', "along-line"),
        ('身', "upright"),
        ('什', "upright"),
        ('戰', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalMixedAlphanumericCode
#[test]
fn vertical_mixed_alphanumeric_code() {
    let orientations = get_glyph_orientations(&shape_line_label("身什戰A1"));
    let expected =
        expect(&[('身', "upright"), ('什', "upright"), ('戰', "upright"), ('A', "upright"), ('1', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalLatinWordAdjoiningCJK
#[test]
fn vertical_latin_word_adjoining_cjk() {
    let orientations = get_glyph_orientations(&shape_line_label("銀座Ginza通り"));
    let expected = expect(&[
        ('銀', "upright"),
        ('座', "upright"),
        ('G', "along-line"),
        ('i', "along-line"),
        ('n', "along-line"),
        ('z', "along-line"),
        ('a', "along-line"),
        ('通', "upright"),
        ('り', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalLongUppercaseWord
#[test]
fn vertical_long_uppercase_word() {
    let orientations = get_glyph_orientations(&shape_line_label("ヴィラ ISHIKAWA"));
    let expected = expect(&[
        ('ヴ', "upright"),
        ('ィ', "upright"),
        ('ラ', "upright"),
        (' ', "along-line"),
        ('I', "along-line"),
        ('S', "along-line"),
        ('H', "along-line"),
        ('I', "along-line"),
        ('K', "along-line"),
        ('A', "along-line"),
        ('W', "along-line"),
        ('A', "along-line"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalShortUppercaseCode
#[test]
fn vertical_short_uppercase_code() {
    let orientations = get_glyph_orientations(&shape_line_label("JR山手線"));
    let expected =
        expect(&[('J', "upright"), ('R', "upright"), ('山', "upright"), ('手', "upright"), ('線', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalPunctuationBetweenDigits
#[test]
fn vertical_punctuation_between_digits() {
    let orientations = get_glyph_orientations(&shape_line_label("国道1-2号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('1', "upright"),
        ('︲', "upright"),
        ('2', "upright"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalPunctuationInRotatedRun
#[test]
fn vertical_punctuation_in_rotated_run() {
    let orientations = get_glyph_orientations(&shape_line_label("国道3.5km"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('3', "along-line"),
        ('.', "along-line"),
        ('5', "along-line"),
        ('k', "along-line"),
        ('m', "along-line"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalMissingGlyphPreservesIndices
#[test]
fn vertical_missing_glyph_preserves_indices() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let hash = font_stack_hash(&font_stack);
    let text = TaggedString::new(u("what 国21号"), SectionOptions::new(1.0, font_stack, GlyphIDType::FontPBF, None));
    let mut glyph_positions = GlyphPositions::new();
    add_stub_glyphs(&text, &mut glyph_positions);
    // Missing "w" must not shift the character index used for the remaining glyphs.
    glyph_positions.get_mut(&hash).unwrap().remove(&GlyphID::from_char('w').unwrap());

    let shaped_line_label = shape_text(
        &text,
        &ShapingOptions {
            max_width: 4096.0,
            line_height: ONE_EM,
            anchor: SymbolAnchorType::TopLeft,
            justify: TextJustifyType::Left,
            spacing: 0.0,
            offset: [0.0, 0.0],
            writing_mode: TextWritingModeType::Vertical,
            allow_vertical_placement: false,
            text_size: ONE_EM,
            text_size_at_bucket_zoom: ONE_EM,
        },
        &glyph_positions,
        &ImagePositions::default(),
    );
    let orientations = get_glyph_orientations(&shaped_line_label);

    let expected = expect(&[
        ('h', "along-line"),
        ('a', "along-line"),
        ('t', "along-line"),
        (' ', "along-line"),
        ('国', "upright"),
        ('2', "upright"),
        ('1', "upright"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.HorizontalGlyphOrientation
#[test]
fn horizontal_glyph_orientation() {
    let orientations =
        get_glyph_orientations(&shape_line_label_with("身什戰33", TextWritingModeType::Horizontal, false));
    let expected = expect(&[
        ('身', "along-line"),
        ('什', "along-line"),
        ('戰', "along-line"),
        ('3', "along-line"),
        ('3', "along-line"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.AllowedVerticalGlyphOrientation
#[test]
fn allowed_vertical_glyph_orientation() {
    let orientations =
        get_glyph_orientations(&shape_line_label_with("two 身什戰", TextWritingModeType::Vertical, true));
    let expected = expect(&[
        ('t', "upright"),
        ('w', "upright"),
        ('o', "upright"),
        (' ', "along-line"),
        ('身', "upright"),
        ('什', "upright"),
        ('戰', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalThreeCharacterUppercaseCode
#[test]
fn vertical_three_character_uppercase_code() {
    let orientations = get_glyph_orientations(&shape_line_label("国道ABC号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('A', "upright"),
        ('B', "upright"),
        ('C', "upright"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalFourCharacterUppercaseWord
#[test]
fn vertical_four_character_uppercase_word() {
    let orientations = get_glyph_orientations(&shape_line_label("国道ABCD号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('A', "along-line"),
        ('B', "along-line"),
        ('C', "along-line"),
        ('D', "along-line"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalBengaliDigits
#[test]
fn vertical_bengali_digits() {
    let orientations = get_glyph_orientations(&shape_line_label("国道১২号"));
    let expected =
        expect(&[('国', "upright"), ('道', "upright"), ('১', "upright"), ('২', "upright"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalArabicIndicDigits
#[test]
fn vertical_arabic_indic_digits() {
    let orientations = get_glyph_orientations(&shape_line_label("国道١٢号"));
    let expected =
        expect(&[('国', "upright"), ('道', "upright"), ('١', "along-line"), ('٢', "along-line"), ('号', "upright")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalDigitWithCombiningMark
#[test]
fn vertical_digit_with_combining_mark() {
    let orientations = get_glyph_orientations(&shape_line_label("国道1\u{0301}号"));
    let expected = expect(&[
        ('国', "upright"),
        ('道', "upright"),
        ('1', "along-line"),
        ('\u{0301}', "along-line"),
        ('号', "upright"),
    ]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalGlyphOrientationRespectsFormattedSections
#[test]
fn vertical_glyph_orientation_respects_formatted_sections() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let mut text = TaggedString::default();
    text.add_text_section(&u("AB"), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));
    text.add_text_section(&u("CD"), SectionOptions::new(0.5, font_stack.clone(), GlyphIDType::FontPBF, None));

    let orientations = get_glyph_orientations(&shape_tagged_text(
        &text,
        TextWritingModeType::Vertical,
        false,
        &ImagePositions::default(),
    ));
    let expected = expect(&[('A', "along-line"), ('B', "along-line"), ('C', "along-line"), ('D', "along-line")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalGlyphOrientationDelimitsInlineImages
#[test]
fn vertical_glyph_orientation_delimits_inline_images() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let mut text = TaggedString::default();
    text.add_text_section(&u("AB"), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));
    text.add_image_section("image", VerticalAlign::Bottom);
    text.add_text_section(&u("CD"), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));

    let image = Impl::new(
        "image".to_string(),
        PremultipliedImage::new(Size::new(12, 24)),
        1.0,
        false,
        vec![],
        vec![],
        None,
        None,
        None,
    )
    .unwrap();
    let mut image_positions = ImagePositions::default();
    image_positions.insert("image".to_string(), ImagePosition::new(RectU16 { x: 0, y: 0, w: 14, h: 26 }, &image));

    let shaped_line_label = shape_tagged_text(&text, TextWritingModeType::Vertical, false, &image_positions);
    assert_eq!(shaped_line_label.lines.len(), 1);
    assert_eq!(shaped_line_label.lines[0].glyphs.len(), 5);
    let orientations = get_glyph_orientations(&shaped_line_label);
    let expected = expect(&[
        ('A', "upright"),
        ('B', "upright"),
        ('\u{E000}', "along-line"), // Inline image placeholder.
        ('C', "upright"),
        ('D', "upright"),
    ]);
    assert_eq!(orientations, expected);
    assert_eq!(shaped_line_label.lines[0].glyphs[2].image_id.as_deref(), Some("image"));
    assert!(shaped_line_label.icons_in_text);
}

// gtest: Shaping.VerticalGlyphOrientationDelimitsNativeGlyphSections
#[test]
fn vertical_glyph_orientation_delimits_native_glyph_sections() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let native_type = face_type("shaping-test-native", "font-stack", &[(0, 0xffff)]);
    let mut text = TaggedString::default();
    text.add_text_section(&u("AB"), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));
    text.add_text_section(&u("C"), SectionOptions::new(1.0, font_stack.clone(), native_type, None));
    text.add_text_section(&u("D"), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));

    let orientations = get_glyph_orientations(&shape_tagged_text(
        &text,
        TextWritingModeType::Vertical,
        false,
        &ImagePositions::default(),
    ));
    let expected = expect(&[('A', "upright"), ('B', "upright"), ('C', "along-line"), ('D', "upright")]);
    assert_eq!(orientations, expected);
}

/// A label with a section of glyphs that HarfBuzz laid out, standing in as
/// "XY", between sections drawn from glyph PBFs.
fn label_with_shaped_section(before: &str, rtl: bool, after: &str, adjusts: Vec<GlyphAdjust>) -> TaggedString {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let shaped_type = face_type("shaping-test-shaped", "font-stack", &[(0, 0xffff)]);
    let mut text = TaggedString::default();
    if !before.is_empty() {
        text.add_text_section(&u(before), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));
    }
    let glyphs =
        vec![GlyphID::with_type(u16::from(b'X'), shaped_type), GlyphID::with_type(u16::from(b'Y'), shaped_type)];
    let shaped = Arc::new(ShapedText { glyphs, adjusts });
    text.add_shaped_section(&shaped, rtl, SectionOptions::new(1.0, font_stack.clone(), shaped_type, None));
    if !after.is_empty() {
        text.add_text_section(&u(after), SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None));
    }
    text
}

fn shaped_codes(shaping: &Shaping) -> String {
    shaping
        .lines
        .iter()
        .flat_map(|line| line.glyphs.iter().map(|glyph| char::from_u32(glyph.glyph.code.into()).unwrap()))
        .collect()
}

fn shape_horizontal(text: &TaggedString) -> Shaping {
    shape_tagged_text(text, TextWritingModeType::Horizontal, false, &ImagePositions::default())
}

// gtest: Shaping.RightToLeftShapedSectionBeforeLatin
#[test]
fn right_to_left_shaped_section_before_latin() {
    // The label reads right to left, so its Latin word lies to the left.
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " Erbil", vec![]));
    assert_eq!(shaped_codes(&shaping), "Erbil XY");
}

// gtest: Shaping.LatinBeforeRightToLeftShapedSection
#[test]
fn latin_before_right_to_left_shaped_section() {
    let shaping = shape_horizontal(&label_with_shaped_section("Erbil ", true, "", vec![]));
    assert_eq!(shaped_codes(&shaping), "Erbil XY");
}

// gtest: Shaping.DigitsAfterRightToLeftShapedSection
#[test]
fn digits_after_right_to_left_shaped_section() {
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " 12", vec![]));
    assert_eq!(shaped_codes(&shaping), "12 XY");
}

// gtest: Shaping.LeftToRightShapedSectionKeepsItsPlace
#[test]
fn left_to_right_shaped_section_keeps_its_place() {
    let shaping = shape_horizontal(&label_with_shaped_section("a ", false, " b", vec![]));
    assert_eq!(shaped_codes(&shaping), "a XY b");
}

// gtest: Shaping.ShapedSectionKeepsItsAdjustmentsWhenItMoves
#[test]
fn shaped_section_keeps_its_adjustments_when_it_moves() {
    let adjusts = vec![GlyphAdjust::new(0.0, 0.0, 5.0), GlyphAdjust::new(0.0, 0.0, 7.0)];
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " 12", adjusts));
    assert_eq!(shaping.lines.len(), 1);
    let glyphs = &shaping.lines[0].glyphs;
    assert_eq!(glyphs.len(), 5);
    // "1", "2", and the space advance 12 each, and X its own 5.
    assert_eq!(glyphs[3].glyph.code, 'X' as u16);
    assert_eq!(glyphs[4].x - glyphs[3].x, 5.0);
}

#[test]
fn a_label_holds_more_than_256_sections() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let face = face_type("shaping-test-sections", "font-stack", &[(0, 0xffff)]);
    let mut text = TaggedString::default();
    for i in 0..300 {
        let type_ = if i % 2 == 0 { GlyphIDType::FontPBF } else { face };
        text.add_text_section(&['a'], SectionOptions::new(1.0, font_stack.clone(), type_, None));
    }
    assert_eq!(text.section_index(299), 299);
    assert_eq!(text.section_of(299).type_, face);
}

// Line breaking measures a shaped section by its shaping.
#[test]
fn shaped_glyphs_take_up_their_width_on_a_line() {
    let adjusts = vec![GlyphAdjust::new(0.0, 0.0, 100.0), GlyphAdjust::new(0.0, 0.0, 100.0)];
    let text = label_with_shaped_section("", false, " ab", adjusts);
    let mut glyph_positions = GlyphPositions::new();
    add_stub_glyphs(&text, &mut glyph_positions);
    let shaping = shape_text(
        &text,
        &ShapingOptions {
            max_width: 150.0,
            line_height: ONE_EM,
            anchor: SymbolAnchorType::TopLeft,
            justify: TextJustifyType::Left,
            spacing: 0.0,
            offset: [0.0, 0.0],
            writing_mode: TextWritingModeType::Horizontal,
            allow_vertical_placement: false,
            text_size: ONE_EM,
            text_size_at_bucket_zoom: ONE_EM,
        },
        &glyph_positions,
        &ImagePositions::default(),
    );
    assert_eq!(shaping.lines.len(), 2);
}

fn setup_shaped_text(text_size: f32) -> Shaping {
    let glyph = PositionedGlyph { glyph: GlyphID::new(32), scale: 1.0, ..Default::default() };
    Shaping {
        lines: vec![PositionedLine { glyphs: vec![glyph], line_offset: 0.0 }],
        bounds: Bounds { right: text_size, bottom: text_size, ..Default::default() },
        ..Default::default()
    }
}

/// The text fit bounds of an icon of `rectangle` with `content`, fitted to
/// `shaped_text` at `font_scale` with `fit`: its `textFitWidth` and
/// `textFitHeight`.
fn text_fit_bounds(
    rectangle: RectU16,
    content: ImageContent,
    fit: [Option<TextFit>; 2],
    shaped_text: &Shaping,
    font_scale: f32,
) -> Bounds {
    let image = ImagePosition::new(
        rectangle,
        &Impl::new(
            "test".to_string(),
            PremultipliedImage::new(Size::new(u32::from(rectangle.w), u32::from(rectangle.h))),
            1.0,
            false,
            vec![],
            vec![],
            Some(content),
            fit[0],
            fit[1],
        )
        .unwrap(),
    );
    let mut shaped_icon = PositionedIcon::new(&image, [0.0, 0.0], SymbolAnchorType::TopLeft);
    shaped_icon.fit_to_text(shaped_text, IconTextFitType::Both, [0.0; 4], [0.0, 0.0], font_scale);
    shaped_icon.text_fit_bounds()
}

// gtest: Shaping.BreakBeforeLeftParenthesis
#[test]
fn break_before_left_parenthesis() {
    let metrics = GlyphMetrics { width: 18, height: 18, left: 2.0, top: -8.0, advance: 21, ..Default::default() };
    let glyph_position = GlyphPosition { metrics, ..Default::default() };

    let font_stack: FontStack = vec!["font-stack".to_string()];
    let section_options = SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, None);
    let mut glyph_positions = GlyphPositions::new();
    for id in ['o', 'a', '(', ')', ' '] {
        let id = GlyphID::from_char(id).unwrap();
        glyph_positions.entry(font_stack_hash(&font_stack)).or_default().entry(id).or_insert(Some(glyph_position));
    }
    let image_positions = ImagePositions::default();

    let glyphs_per_line = |text: &str| {
        let shaping = shape_text(
            &TaggedString::new(u(text), section_options.clone()),
            &ShapingOptions {
                max_width: 5.0 * ONE_EM,
                line_height: ONE_EM,
                anchor: SymbolAnchorType::Center,
                justify: TextJustifyType::Center,
                spacing: 0.0,
                offset: [0.0, 0.0],
                writing_mode: TextWritingModeType::Horizontal,
                allow_vertical_placement: false,
                text_size: 16.0,
                text_size_at_bucket_zoom: 16.0,
            },
            &glyph_positions,
            &image_positions,
        );
        shaping.lines.iter().map(|line| line.glyphs.len()).collect::<Vec<usize>>()
    };

    // The break goes before the parenthesis, so it opens the second line
    // instead of closing the first.
    assert_eq!(glyphs_per_line("oooooooo(aaaaaaaaa)"), vec![8, 11]);
    assert_eq!(glyphs_per_line("oooooooo (aaaaaaaaa)"), vec![8, 11]);
    // A trailing parenthesis never becomes a line of its own.
    assert_eq!(glyphs_per_line("oooooooo("), vec![9]);
}

// gtest: Shaping.applyTextFit
#[test]
fn apply_text_fit() {
    let text_size = 4.0f32;
    let font_scale = 4.0f32;
    let expected_image_size = text_size * font_scale;
    let shaped_text = setup_shaped_text(text_size);

    {
        // applyTextFitHorizontal
        // This set of tests against applyTextFit starts with a 100x20 image
        // with a 5,5,95,15 content box that has been fitted to a 4*4 text
        // with scale 4, resulting in a 16*16 image.
        let horizontal_rectangle = RectU16 { x: 0, y: 0, w: 100, h: 20 };
        let horizontal_content = ImageContent { left: 5.0, top: 5.0, right: 95.0, bottom: 15.0 };

        // applyTextFit: not specified
        // No change should happen
        assert_eq!(
            text_fit_bounds(horizontal_rectangle, horizontal_content, [None, None], &shaped_text, font_scale),
            Bounds { left: 0.0, top: 0.0, right: expected_image_size, bottom: expected_image_size }
        );

        // applyTextFit: both stretchOrShrink
        // No change should happen
        assert_eq!(
            text_fit_bounds(
                horizontal_rectangle,
                horizontal_content,
                [Some(TextFit::StretchOrShrink), Some(TextFit::StretchOrShrink)],
                &shaped_text,
                font_scale
            ),
            Bounds { left: 0.0, top: 0.0, right: expected_image_size, bottom: expected_image_size }
        );

        // applyTextFit: stretchOnly, proportional
        // Since textFitWidth is stretchOnly, it should be returned to the
        // aspect ratio of the content rectangle (9:1) aspect ratio so 144x16.
        assert_eq!(
            text_fit_bounds(
                horizontal_rectangle,
                horizontal_content,
                [Some(TextFit::StretchOnly), Some(TextFit::Proportional)],
                &shaped_text,
                font_scale
            ),
            Bounds { left: 0.0, top: 0.0, right: expected_image_size * 9.0, bottom: expected_image_size }
        );
    }

    {
        // applyTextFitVertical
        // This set of tests against applyTextFit starts with a 20x100 image
        // with a 5,5,15,95 content box that has been fitted to a 4*4 text
        // with scale 4, resulting in a 16*16 image.
        let vertical_rectangle = RectU16 { x: 0, y: 0, w: 20, h: 100 };
        let vertical_content = ImageContent { left: 5.0, top: 5.0, right: 15.0, bottom: 95.0 };

        // applyTextFit: stretchOnly, proportional
        // Since textFitWidth is stretchOnly, it should be returned to the
        // aspect ratio of the content rectangle (9:1) aspect ratio so 144x16.
        assert_eq!(
            text_fit_bounds(
                vertical_rectangle,
                vertical_content,
                [Some(TextFit::Proportional), Some(TextFit::StretchOnly)],
                &shaped_text,
                font_scale
            ),
            Bounds { left: 0.0, top: 0.0, right: expected_image_size, bottom: expected_image_size * 9.0 }
        );
    }
}
