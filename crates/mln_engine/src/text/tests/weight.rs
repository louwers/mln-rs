// Variable fonts draw at regular weight, and device fonts at the weight a
// font stack names. The fixtures are Noto Sans CJK JP's
// variable font, whose default instance is Thin (100), subset to the glyph
// of "中" and "A", whose advance varies, by fontTools (`fonttools subset
// NotoSansCJK-VF.ttc --font-number=0 --text=中A --layout-features='*'
// --name-IDs='*'`), and its static instances
// at 400 and 100 (`fonttools varLib.instancer cjk-vf.otf wght=400`).

use std::num::NonZeroU16;

use crate::text::glyph::{Glyph, GlyphIDType};
use crate::text::weight::{REGULAR_WEIGHT, at_weight};
use crate::text::{GlyphAdjust, ShapedText, Shaper};

const VARIABLE: &[u8] = include_bytes!("../../../tests/fixtures/cjk-vf.otf");
const REGULAR: &[u8] = include_bytes!("../../../tests/fixtures/cjk-400.otf");
const THIN: &[u8] = include_bytes!("../../../tests/fixtures/cjk-100.otf");
const KHMER: &[u8] = include_bytes!("../../../tests/fixtures/khmer.ttf");

/// The positions and glyphs of "中A", shaped and rasterized.
fn shape(font: &[u8]) -> (Vec<GlyphAdjust>, Vec<Glyph>) {
    let shaper = Shaper::new(GlyphIDType::face(NonZeroU16::MIN), font.to_vec());
    assert!(shaper.valid());
    let mut shaped = ShapedText::default();
    shaper.shape(&['中', 'A'], false, &mut shaped);
    assert_eq!(shaped.glyphs.len(), 2);
    let glyphs = shaped.glyphs.iter().map(|&id| shaper.rasterize_glyph(id)).collect();
    (shaped.adjusts, glyphs)
}

/// The glyph's coverage: the sum of its alpha values.
fn ink(glyph: &Glyph) -> u64 {
    glyph.bitmap.data.iter().map(|&alpha| u64::from(alpha)).sum()
}

#[test]
fn a_variable_font_draws_as_its_regular_instance() {
    let (variable, regular, thin) = (shape(VARIABLE), shape(REGULAR), shape(THIN));
    // "A" is wider at regular weight than at Thin. The static instance's
    // advances, like its outlines, are rounded to whole units.
    let advances = |adjusts: &[GlyphAdjust]| adjusts.iter().map(|adjust| adjust.advance).collect::<Vec<_>>();
    for ((variable, regular), thin) in advances(&variable.0).iter().zip(advances(&regular.0)).zip(advances(&thin.0)) {
        assert!((variable - regular).abs() <= 1.0 / 64.0, "{variable} vs {regular}");
        assert!(thin == regular || (thin - regular).abs() > 0.5, "{thin} vs {regular}");
    }
    assert_ne!(advances(&thin.0), advances(&regular.0));
    for ((variable, regular), thin) in variable.1.iter().zip(&regular.1).zip(&thin.1) {
        assert_eq!(variable.metrics, regular.metrics);
        let (variable, regular, thin) = (ink(variable), ink(regular), ink(thin));
        assert!(variable.abs_diff(regular) * 100 < regular, "{variable} vs {regular}");
        assert!(thin * 3 < regular * 2, "{thin} vs {regular}");
    }
}

#[test]
fn only_a_font_off_regular_weight_gets_a_location() {
    let font = |data: &'static [u8]| skrifa::Font::new(data, 0).unwrap();
    assert_ne!(at_weight(font(VARIABLE), REGULAR_WEIGHT).normalized_coords(), []);
    assert_eq!(at_weight(font(REGULAR), REGULAR_WEIGHT).normalized_coords(), []);
    assert_eq!(at_weight(font(KHMER), REGULAR_WEIGHT).normalized_coords(), []);
}

/// The glyph of "中" that `font` draws, loaded at `weight`.
fn drawn_at(font: &[u8], weight: u16) -> Glyph {
    let shaper = Shaper::at_weight(GlyphIDType::face(NonZeroU16::MIN), font.to_vec(), f32::from(weight));
    shaper.rasterize_glyph(shaper.glyph_of('中').expect("a glyph for 中"))
}

#[test]
fn a_variable_font_loaded_at_a_weight_draws_at_it() {
    let close = |a: &Glyph, b: &Glyph| ink(a).abs_diff(ink(b)) * 100 < ink(b);
    assert!(close(&drawn_at(VARIABLE, 100), &drawn_at(THIN, 100)));
    assert!(close(&drawn_at(VARIABLE, 400), &drawn_at(REGULAR, 400)));
    // Below the axis, the font draws at its lightest; a static font draws
    // at its own weight whatever is asked for.
    assert_eq!(ink(&drawn_at(VARIABLE, 1)), ink(&drawn_at(VARIABLE, 100)));
    assert_eq!(ink(&drawn_at(REGULAR, 100)), ink(&drawn_at(REGULAR, 900)));
    assert!(ink(&drawn_at(VARIABLE, 900)) > ink(&drawn_at(VARIABLE, 400)));
}
