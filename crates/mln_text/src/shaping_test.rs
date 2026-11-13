use std::sync::Arc;

use mln_style_types::image::{ImageContent, ImagePosition, ImagePositions, Impl, RectU16, TextFit};
use mln_style_types::types::{IconTextFitType, SymbolAnchorType, TextJustifyType};
use mln_util::constants::ONE_EM;
use mln_util::font_stack::FontStack;
use mln_util::i18n;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;

use crate::bidi::BiDi;
use crate::glyph::{
    Glyph, GlyphID, GlyphIDType, GlyphMap, GlyphMetrics, GlyphPosition, GlyphPositions, PositionedGlyph,
    PositionedLine, Shaping, WritingModeType, gen_new_glyph_id_type,
};
use crate::harfbuzz::HBShapeAdjust;
use crate::shaping::{PositionedIcon, get_shaping};
use crate::tagged_string::{FontStackHash, SectionOptions, TaggedString, font_stack_hash};

fn u(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn add_stub_glyph(
    glyphs: &mut GlyphMap,
    glyph_positions: &mut GlyphPositions,
    font_stack_hash: FontStackHash,
    code_point: u16,
    type_: GlyphIDType,
) {
    let glyph_id = GlyphID::with_type(code_point, type_);
    let metrics = GlyphMetrics { width: 10, height: 18, left: 1.0, top: -8.0, advance: 12, ..Default::default() };

    let glyph = Glyph { id: glyph_id, metrics, ..Default::default() };
    glyphs.entry(font_stack_hash).or_default().insert(glyph_id, Some(Arc::new(glyph)));

    let position = GlyphPosition { metrics, ..Default::default() };
    glyph_positions.entry(font_stack_hash).or_default().insert(glyph_id, position);
}

fn add_stub_glyphs(string: &TaggedString, glyphs: &mut GlyphMap, glyph_positions: &mut GlyphPositions) {
    for i in 0..string.length() {
        let code_point = string.get_char_code_at(i);

        let section = string.get_section(i);
        if section.image_id.is_some() {
            continue;
        }

        add_stub_glyph(glyphs, glyph_positions, section.font_stack_hash, code_point, section.type_);
        if section.type_ == GlyphIDType::FontPBF {
            let vertical_code_point = i18n::verticalize_punctuation(code_point);
            if vertical_code_point != 0 {
                add_stub_glyph(glyphs, glyph_positions, section.font_stack_hash, vertical_code_point, section.type_);
            }
        }
    }
}

fn shape_tagged_text(
    string: &TaggedString,
    writing_mode: WritingModeType,
    allow_vertical_placement: bool,
    image_positions: &ImagePositions,
) -> Shaping {
    let mut glyphs = GlyphMap::new();
    let mut glyph_positions = GlyphPositions::new();
    add_stub_glyphs(string, &mut glyphs, &mut glyph_positions);

    let mut bidi = BiDi::new();
    get_shaping(
        string,
        4096.0,
        ONE_EM,
        SymbolAnchorType::TopLeft,
        TextJustifyType::Left,
        0.0,
        &[0.0, 0.0],
        writing_mode,
        &mut bidi,
        &glyphs,
        &glyph_positions,
        image_positions,
        ONE_EM,
        ONE_EM,
        allow_vertical_placement,
    )
    .unwrap()
}

fn shape_line_label_with(text: &str, writing_mode: WritingModeType, allow_vertical_placement: bool) -> Shaping {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    shape_tagged_text(
        &TaggedString::new(u(text), SectionOptions::new(1.0, font_stack, GlyphIDType::FontPBF, 0, None)),
        writing_mode,
        allow_vertical_placement,
        &ImagePositions::new(),
    )
}

fn shape_line_label(text: &str) -> Shaping {
    shape_line_label_with(text, WritingModeType::Vertical, false)
}

type GlyphOrientation = (u16, &'static str);

fn get_glyph_orientations(shaping: &Shaping) -> Vec<GlyphOrientation> {
    let mut result = Vec::new();
    for line in &shaping.positioned_lines {
        for glyph in &line.positioned_glyphs {
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

    let glyph = Glyph { id: GlyphID::from('中' as u16), metrics, ..Default::default() };

    let mut bidi = BiDi::new();
    let immutable_glyph = Arc::new(glyph);
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let section_options = SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, 0, None);
    let layout_text_size = 16.0f32;
    let layout_text_size_at_bucket_zoom_level = 16.0f32;
    let mut glyphs = GlyphMap::new();
    glyphs.entry(font_stack_hash(&font_stack)).or_default().insert(GlyphID::from('中' as u16), Some(immutable_glyph));
    let mut glyph_positions = GlyphPositions::new();
    glyph_positions.entry(font_stack_hash(&font_stack)).or_default().insert(GlyphID::from('中' as u16), glyph_position);
    let image_positions = ImagePositions::new();

    let mut test_get_shaping = |string: &TaggedString, max_width_in_chars: u32| {
        get_shaping(
            string,
            max_width_in_chars as f32 * ONE_EM,
            ONE_EM, // lineHeight
            SymbolAnchorType::Center,
            TextJustifyType::Center,
            0.0,         // spacing
            &[0.0, 0.0], // translate
            WritingModeType::Horizontal,
            &mut bidi,
            &glyphs,
            &glyph_positions,
            &image_positions,
            layout_text_size,
            layout_text_size_at_bucket_zoom_level,
            /*allowVerticalPlacement*/ false,
        )
        .unwrap()
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
        assert_eq!(shaping.positioned_lines.len(), 3);
        assert_eq!(shaping.top, -36.0);
        assert_eq!(shaping.bottom, 36.0);
        assert_eq!(shaping.left, -63.0);
        assert_eq!(shaping.right, 63.0);
        assert_eq!(shaping.writing_mode, WritingModeType::Horizontal);
    }

    // 2 lines
    // 中中
    // 中
    {
        let string = TaggedString::new(u("中中\u{200b}中"), section_options.clone());
        let shaping = test_get_shaping(&string, 1);
        assert_eq!(shaping.positioned_lines.len(), 2);
        assert_eq!(shaping.top, -24.0);
        assert_eq!(shaping.bottom, 24.0);
        assert_eq!(shaping.left, -21.0);
        assert_eq!(shaping.right, 21.0);
        assert_eq!(shaping.writing_mode, WritingModeType::Horizontal);
    }

    // 1 line
    // 中中
    {
        let string = TaggedString::new(u("中中\u{200b}"), section_options.clone());
        let shaping = test_get_shaping(&string, 2);
        assert_eq!(shaping.positioned_lines.len(), 1);
        assert_eq!(shaping.top, -12.0);
        assert_eq!(shaping.bottom, 12.0);
        assert_eq!(shaping.left, -21.0);
        assert_eq!(shaping.right, 21.0);
        assert_eq!(shaping.writing_mode, WritingModeType::Horizontal);
    }

    // 5 'new' lines.
    {
        let string = TaggedString::new(u("\u{200b}\u{200b}\u{200b}\u{200b}\u{200b}"), section_options);
        let shaping = test_get_shaping(&string, 1);
        assert_eq!(shaping.positioned_lines.len(), 5);
        assert_eq!(shaping.top, -60.0);
        assert_eq!(shaping.bottom, 60.0);
        assert_eq!(shaping.left, 0.0);
        assert_eq!(shaping.right, 0.0);
        assert_eq!(shaping.writing_mode, WritingModeType::Horizontal);
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
    let text = TaggedString::new(u("what 国21号"), SectionOptions::new(1.0, font_stack, GlyphIDType::FontPBF, 0, None));
    let mut glyphs = GlyphMap::new();
    let mut glyph_positions = GlyphPositions::new();
    add_stub_glyphs(&text, &mut glyphs, &mut glyph_positions);
    // Missing "w" must not shift the character index used for the remaining glyphs.
    glyphs.get_mut(&hash).unwrap().remove(&GlyphID::from('w' as u16));
    glyph_positions.get_mut(&hash).unwrap().remove(&GlyphID::from('w' as u16));

    let mut bidi = BiDi::new();
    let shaped_line_label = get_shaping(
        &text,
        4096.0,
        ONE_EM,
        SymbolAnchorType::TopLeft,
        TextJustifyType::Left,
        0.0,
        &[0.0, 0.0],
        WritingModeType::Vertical,
        &mut bidi,
        &glyphs,
        &glyph_positions,
        &ImagePositions::new(),
        ONE_EM,
        ONE_EM,
        false,
    )
    .unwrap();
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
    let orientations = get_glyph_orientations(&shape_line_label_with("身什戰33", WritingModeType::Horizontal, false));
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
    let orientations = get_glyph_orientations(&shape_line_label_with("two 身什戰", WritingModeType::Vertical, true));
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
    text.add_text_section(&u("AB"), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);
    text.add_text_section(&u("CD"), 0.5, &font_stack, GlyphIDType::FontPBF, true, None);

    let orientations =
        get_glyph_orientations(&shape_tagged_text(&text, WritingModeType::Vertical, false, &ImagePositions::new()));
    let expected = expect(&[('A', "along-line"), ('B', "along-line"), ('C', "along-line"), ('D', "along-line")]);
    assert_eq!(orientations, expected);
}

// gtest: Shaping.VerticalGlyphOrientationDelimitsInlineImages
#[test]
fn vertical_glyph_orientation_delimits_inline_images() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let mut text = TaggedString::default();
    text.add_text_section(&u("AB"), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);
    text.add_image_section("image");
    text.add_text_section(&u("CD"), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);

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
    let mut image_positions = ImagePositions::new();
    image_positions.insert("image".to_string(), ImagePosition::new(RectU16 { x: 0, y: 0, w: 14, h: 26 }, &image, 0));

    let shaped_line_label = shape_tagged_text(&text, WritingModeType::Vertical, false, &image_positions);
    assert_eq!(shaped_line_label.positioned_lines.len(), 1);
    assert_eq!(shaped_line_label.positioned_lines[0].positioned_glyphs.len(), 5);
    let orientations = get_glyph_orientations(&shaped_line_label);
    let expected = expect(&[
        ('A', "upright"),
        ('B', "upright"),
        ('\u{E000}', "along-line"), // Inline image placeholder.
        ('C', "upright"),
        ('D', "upright"),
    ]);
    assert_eq!(orientations, expected);
    assert_eq!(shaped_line_label.positioned_lines[0].positioned_glyphs[2].image_id.as_deref(), Some("image"));
    assert!(shaped_line_label.icons_in_text);
}

// gtest: Shaping.VerticalGlyphOrientationDelimitsNativeGlyphSections
#[test]
fn vertical_glyph_orientation_delimits_native_glyph_sections() {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let native_type = gen_new_glyph_id_type("shaping-test-native", &font_stack, &[(0, 0xffff)]);
    let mut text = TaggedString::default();
    text.add_text_section(&u("AB"), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);
    text.add_text_section(&u("C"), 1.0, &font_stack, native_type, true, None);
    text.add_text_section(&u("D"), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);

    let orientations =
        get_glyph_orientations(&shape_tagged_text(&text, WritingModeType::Vertical, false, &ImagePositions::new()));
    let expected = expect(&[('A', "upright"), ('B', "upright"), ('C', "along-line"), ('D', "upright")]);
    assert_eq!(orientations, expected);
}

/// A label with a section of glyphs that HarfBuzz laid out, standing in as
/// "XY", between sections drawn from glyph PBFs.
fn label_with_shaped_section(
    before: &str,
    rtl: bool,
    after: &str,
    adjusts: Option<&Arc<Vec<HBShapeAdjust>>>,
) -> TaggedString {
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let shaped_type = gen_new_glyph_id_type("shaping-test-shaped", &font_stack, &[(0, 0xffff)]);
    let mut text = TaggedString::default();
    if !before.is_empty() {
        text.add_text_section(&u(before), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);
    }
    text.add_text_section_with_adjusts(&u("XY"), 1.0, &font_stack, shaped_type, &adjusts.cloned(), rtl, true, None);
    if !after.is_empty() {
        text.add_text_section(&u(after), 1.0, &font_stack, GlyphIDType::FontPBF, true, None);
    }
    text
}

fn shaped_codes(shaping: &Shaping) -> Vec<u16> {
    shaping
        .positioned_lines
        .iter()
        .flat_map(|line| line.positioned_glyphs.iter().map(|glyph| glyph.glyph.code))
        .collect()
}

fn shape_horizontal(text: &TaggedString) -> Shaping {
    shape_tagged_text(text, WritingModeType::Horizontal, false, &ImagePositions::new())
}

// gtest: Shaping.RightToLeftShapedSectionBeforeLatin
#[test]
fn right_to_left_shaped_section_before_latin() {
    // The label reads right to left, so its Latin word lies to the left.
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " Erbil", None));
    assert_eq!(shaped_codes(&shaping), u("Erbil XY"));
}

// gtest: Shaping.LatinBeforeRightToLeftShapedSection
#[test]
fn latin_before_right_to_left_shaped_section() {
    let shaping = shape_horizontal(&label_with_shaped_section("Erbil ", true, "", None));
    assert_eq!(shaped_codes(&shaping), u("Erbil XY"));
}

// gtest: Shaping.DigitsAfterRightToLeftShapedSection
#[test]
fn digits_after_right_to_left_shaped_section() {
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " 12", None));
    assert_eq!(shaped_codes(&shaping), u("12 XY"));
}

// gtest: Shaping.LeftToRightShapedSectionKeepsItsPlace
#[test]
fn left_to_right_shaped_section_keeps_its_place() {
    let shaping = shape_horizontal(&label_with_shaped_section("a ", false, " b", None));
    assert_eq!(shaped_codes(&shaping), u("a XY b"));
}

// gtest: Shaping.ShapedSectionKeepsItsAdjustmentsWhenItMoves
#[test]
fn shaped_section_keeps_its_adjustments_when_it_moves() {
    let adjusts = Arc::new(vec![HBShapeAdjust::new(0.0, 0.0, 5.0), HBShapeAdjust::new(0.0, 0.0, 7.0)]);
    let shaping = shape_horizontal(&label_with_shaped_section("", true, " 12", Some(&adjusts)));
    assert_eq!(shaping.positioned_lines.len(), 1);
    let glyphs = &shaping.positioned_lines[0].positioned_glyphs;
    assert_eq!(glyphs.len(), 5);
    // "1", "2", and the space advance 12 each, and X its own 5.
    assert_eq!(glyphs[3].glyph.code, 'X' as u16);
    assert_eq!(glyphs[4].x - glyphs[3].x, 5.0);
}

fn setup_shaped_text(shaped_text: &mut Shaping, text_size: f32) {
    let glyph = PositionedGlyph::new(
        GlyphID::new(32),
        0.0,
        0.0,
        false,
        0,
        1.0,
        /*texRect*/ RectU16::default(),
        /*metrics*/ GlyphMetrics::default(),
        /*imageID*/ None,
        0,
    );
    shaped_text.right = text_size;
    shaped_text.bottom = text_size;
    shaped_text.positioned_lines.push(PositionedLine::default());
    shaped_text.positioned_lines.last_mut().unwrap().positioned_glyphs.push(glyph);
}

#[expect(clippy::too_many_arguments, reason = "a case's image, text fit, shaping and scale, and the expected edges")]
fn test_apply_text_fit(
    rectangle: RectU16,
    content: ImageContent,
    text_fit_width: Option<TextFit>,
    text_fit_height: Option<TextFit>,
    shaped_text: &Shaping,
    font_scale: f32,
    expected_right: f32,
    expected_bottom: f32,
) {
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
            text_fit_width,
            text_fit_height,
        )
        .unwrap(),
        0,
    );
    let mut shaped_icon = PositionedIcon::shape_icon(&image, &[0.0, 0.0], SymbolAnchorType::TopLeft);
    shaped_icon.fit_icon_to_text(shaped_text, IconTextFitType::Both, &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0], font_scale);
    let icon = shaped_icon.apply_text_fit();
    assert_eq!(icon.top(), 0.0);
    assert_eq!(icon.left(), 0.0);
    assert_eq!(icon.right(), expected_right);
    assert_eq!(icon.bottom(), expected_bottom);
}

// gtest: Shaping.BreakBeforeLeftParenthesis
#[test]
fn break_before_left_parenthesis() {
    let metrics = GlyphMetrics { width: 18, height: 18, left: 2.0, top: -8.0, advance: 21, ..Default::default() };
    let glyph_position = GlyphPosition { metrics, ..Default::default() };

    let mut bidi = BiDi::new();
    let font_stack: FontStack = vec!["font-stack".to_string()];
    let section_options = SectionOptions::new(1.0, font_stack.clone(), GlyphIDType::FontPBF, 0, None);
    let mut glyphs = GlyphMap::new();
    let mut glyph_positions = GlyphPositions::new();
    for id in ['o', 'a', '(', ')', ' '] {
        let id = GlyphID::from(id as u16);
        let glyph = Glyph { id, metrics, ..Default::default() };
        glyphs.entry(font_stack_hash(&font_stack)).or_default().entry(id).or_insert(Some(Arc::new(glyph)));
        glyph_positions.entry(font_stack_hash(&font_stack)).or_default().entry(id).or_insert(glyph_position);
    }
    let image_positions = ImagePositions::new();

    let mut glyphs_per_line = |text: &str| {
        let shaping = get_shaping(
            &TaggedString::new(u(text), section_options.clone()),
            5.0 * ONE_EM,
            ONE_EM, // lineHeight
            SymbolAnchorType::Center,
            TextJustifyType::Center,
            0.0,         // spacing
            &[0.0, 0.0], // translate
            WritingModeType::Horizontal,
            &mut bidi,
            &glyphs,
            &glyph_positions,
            &image_positions,
            16.0, // layoutTextSize
            16.0, // layoutTextSizeAtBucketZoomLevel
            /*allowVerticalPlacement*/ false,
        )
        .unwrap();
        shaping.positioned_lines.iter().map(|line| line.positioned_glyphs.len()).collect::<Vec<usize>>()
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
    let mut shaped_text = Shaping::default();
    setup_shaped_text(&mut shaped_text, text_size);

    {
        // applyTextFitHorizontal
        // This set of tests against applyTextFit starts with a 100x20 image
        // with a 5,5,95,15 content box that has been fitted to a 4*4 text
        // with scale 4, resulting in a 16*16 image.
        let horizontal_rectangle = RectU16 { x: 0, y: 0, w: 100, h: 20 };
        let horizontal_content = ImageContent { left: 5.0, top: 5.0, right: 95.0, bottom: 15.0 };

        // applyTextFit: not specified
        // No change should happen
        test_apply_text_fit(
            horizontal_rectangle,
            horizontal_content,
            None,
            None,
            &shaped_text,
            font_scale,
            expected_image_size,
            expected_image_size,
        );

        // applyTextFit: both stretchOrShrink
        // No change should happen
        test_apply_text_fit(
            horizontal_rectangle,
            horizontal_content,
            Some(TextFit::StretchOrShrink),
            Some(TextFit::StretchOrShrink),
            &shaped_text,
            font_scale,
            expected_image_size,
            expected_image_size,
        );

        // applyTextFit: stretchOnly, proportional
        // Since textFitWidth is stretchOnly, it should be returned to the
        // aspect ratio of the content rectangle (9:1) aspect ratio so 144x16.
        test_apply_text_fit(
            horizontal_rectangle,
            horizontal_content,
            Some(TextFit::StretchOnly),
            Some(TextFit::Proportional),
            &shaped_text,
            font_scale,
            expected_image_size * 9.0,
            expected_image_size,
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
        test_apply_text_fit(
            vertical_rectangle,
            vertical_content,
            Some(TextFit::Proportional),
            Some(TextFit::StretchOnly),
            &shaped_text,
            font_scale,
            expected_image_size,
            expected_image_size * 9.0,
        );
    }
}
