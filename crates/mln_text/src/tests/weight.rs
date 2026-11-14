// Variable fonts draw at regular weight. The fixtures are Noto Sans CJK JP's
// variable font, whose default instance is Thin (100), subset to the glyph
// of "中" and "A", whose advance varies, by fontTools (`fonttools subset
// NotoSansCJK-VF.ttc --font-number=0 --text=中A --layout-features='*'
// --name-IDs='*'`), and its static instances
// at 400 and 100 (`fonttools varLib.instancer cjk-vf.otf wght=400`).

use crate::glyph::{Glyph, GlyphIDType};
use crate::weight::at_regular_weight;
use crate::{FreeTypeLibrary, HBShapeAdjust, HBShaper};

const VARIABLE: &[u8] = include_bytes!("../../tests/fixtures/cjk-vf.otf");
const REGULAR: &[u8] = include_bytes!("../../tests/fixtures/cjk-400.otf");
const THIN: &[u8] = include_bytes!("../../tests/fixtures/cjk-100.otf");
const KHMER: &[u8] = include_bytes!("../../tests/fixtures/khmer.ttf");

/// The positions and glyphs of "中A", shaped and rasterized.
fn shape(font: &[u8]) -> (Vec<HBShapeAdjust>, Vec<Glyph>) {
    let shaper = HBShaper::new(GlyphIDType(1), font.to_vec(), &FreeTypeLibrary::new());
    assert!(shaper.valid());
    let (mut ids, mut adjusts) = (Vec::new(), Vec::new());
    shaper.create_complex_glyph_ids(&[0x4E2D, 0x41], false, &mut ids, &mut adjusts);
    assert_eq!(ids.len(), 2);
    (adjusts, ids.iter().map(|id| shaper.rasterize_glyph(id)).collect())
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
    let advances = |adjusts: &[HBShapeAdjust]| adjusts.iter().map(|adjust| adjust.advance).collect::<Vec<_>>();
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
    assert_ne!(at_regular_weight(font(VARIABLE)).normalized_coords(), []);
    assert_eq!(at_regular_weight(font(REGULAR)).normalized_coords(), []);
    assert_eq!(at_regular_weight(font(KHMER)).normalized_coords(), []);
}
